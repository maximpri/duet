// SPDX-License-Identifier: GPL-3.0-or-later
//! The client against scripted servers over both transports.

use declass_boundary::third_party::Guard;
use declass_boundary::view::{PassThrough, Presenter};
use declass_mcp::mock::{Answer, HttpMock, Mock};
use declass_mcp::{Client, McpError, Transport};
use serde_json::{Map, Value, json};
use std::time::Duration;

const T: Duration = Duration::from_secs(5);

fn args(v: Value) -> Map<String, Value> {
    let Value::Object(m) = v else { unreachable!() };
    m
}

async fn stdio_client(mock: &Mock, ping_first: bool) -> Client {
    let (input, output) = mock.stdio(ping_first);
    Client::connect(Transport::stdio(input, output), T)
        .await
        .unwrap()
}

#[tokio::test]
async fn stdio_handshake_listing_and_calls() {
    let mock = Mock::default();
    // The server interleaves its own requests, notifications and junk lines.
    let mut c = stdio_client(&mock, true).await;
    assert_eq!(c.protocol_version, "2025-06-18");
    assert_eq!(c.server_name, "mock");
    let tools = c.list_tools(T).await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    // Eight tools over four pages.
    assert_eq!(
        names,
        [
            "echo", "write", "fail", "boom", "slow", "crash", "picture", "canned"
        ]
    );
    assert!(tools[0].read_only && !tools[1].read_only);
    assert_eq!(tools[0].input_schema["type"], "object");

    let r = c
        .call_tool("echo", &args(json!({"text": "hi"})), T)
        .await
        .unwrap();
    assert_eq!((r.is_error, r.text.as_str()), (false, "hi"));
    let r = c
        .call_tool("fail", &args(json!({"text": "x"})), T)
        .await
        .unwrap();
    assert!(r.is_error && r.text.contains("cannot do that"));
    let e = c
        .call_tool("boom", &args(json!({"text": "y"})), T)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, McpError::Rpc { code: -32000, message } if message == "exploded on y"),
        "{e:?}"
    );
    let r = c.call_tool("picture", &Map::new(), T).await.unwrap();
    assert_eq!(r.text, "[image: image/png, 8 bytes; not shown]");

    let methods = mock.methods();
    let at = |m: &str| methods.iter().position(|x| x == m).unwrap();
    assert!(at("initialize") == 0 && at("notifications/initialized") < at("tools/list"));
    // The client answered the server's pings (they appear as answers, without a method).
    assert!(methods.iter().any(String::is_empty), "{methods:?}");
}

#[tokio::test]
async fn a_timed_out_call_is_cancelled_and_the_next_call_still_works() {
    let mock = Mock::default();
    let mut c = stdio_client(&mock, false).await;
    let e = c
        .call_tool(
            "slow",
            &args(json!({"seconds": 2})),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
    assert_eq!(e, McpError::Timeout(1));
    // The late answer to the cancelled request is skipped.
    let r = c
        .call_tool("echo", &args(json!({"text": "after"})), T)
        .await
        .unwrap();
    assert_eq!(r.text, "after");
    assert!(
        mock.methods()
            .contains(&"notifications/cancelled".to_owned())
    );
}

#[tokio::test]
async fn a_crashed_server_is_reported_as_closed() {
    let mock = Mock::default();
    let mut c = stdio_client(&mock, false).await;
    let e = c.call_tool("crash", &Map::new(), T).await.unwrap_err();
    assert!(matches!(e, McpError::Closed(_)), "{e:?}");
    let again = c.call_tool("echo", &Map::new(), T).await.unwrap_err();
    assert!(matches!(again, McpError::Closed(_)), "{again:?}");
}

#[tokio::test]
async fn an_unknown_protocol_revision_is_refused() {
    let mock = Mock {
        version: Some("1999-01-01".into()),
        ..Mock::default()
    };
    let (input, output) = mock.stdio(false);
    let Err(e) = Client::connect(Transport::stdio(input, output), T).await else {
        panic!("connected")
    };
    assert!(e.to_string().contains("1999-01-01"), "{e}");
    // An older revision the client knows is accepted.
    let mock = Mock {
        version: Some("2024-11-05".into()),
        ..Mock::default()
    };
    assert_eq!(
        stdio_client(&mock, false).await.protocol_version,
        "2024-11-05"
    );
}

#[tokio::test]
async fn http_json_and_event_stream_answers_with_a_session() {
    for answer in [Answer::Json, Answer::EventStream] {
        let mock = Mock::default();
        let server = HttpMock::start(mock.clone(), answer).await;
        let t = Transport::http(
            &server.url,
            &[("Authorization".into(), "Bearer t0ken".into())],
            open(),
        )
        .unwrap();
        let mut c = Client::connect(t, T).await.unwrap();
        assert_eq!(c.list_tools(T).await.unwrap().len(), 8);
        let r = c
            .call_tool("echo", &args(json!({"text": "over http"})), T)
            .await
            .unwrap();
        assert_eq!(r.text, "over http");

        // A restarted server forgets the session: the client initializes again.
        *server.expire_next.lock().unwrap() = true;
        let r = c
            .call_tool("echo", &args(json!({"text": "again"})), T)
            .await
            .unwrap();
        assert_eq!(r.text, "again");
        c.close().await;

        let seen = server.seen();
        let first = &seen[0];
        assert!(first.session.is_none() && first.version.is_none());
        assert!(
            seen.iter()
                .all(|s| s.authorization.as_deref() == Some("Bearer t0ken"))
        );
        // Everything but initialize carries the session and the revision.
        assert!(
            seen.iter()
                // (Answers to the server's pings during initialize precede the revision.)
                .filter(|s| !s.body.contains("\"initialize\"") && !s.body.contains("\"result\""))
                .all(|s| s.session.is_some() && s.version.as_deref() == Some("2025-06-18")),
            "{answer:?}: {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|s| s.session.as_deref() == Some("session-2"))
        );
        assert_eq!(seen.last().unwrap().method, "DELETE");
        if answer == Answer::EventStream {
            // The ping inside the stream was answered with a separate POST.
            assert!(seen.iter().any(|s| s.body.contains("\"id\":\"s1\"")));
        }
    }
}

#[tokio::test]
async fn an_unreachable_http_server_is_closed_and_errors_are_short() {
    let t = Transport::http("http://127.0.0.1:9/mcp", &[], open()).unwrap();
    let Err(e) = Client::connect(t, T).await else {
        panic!("connected")
    };
    assert!(matches!(e, McpError::Closed(_)), "{e:?}");
    assert!(Transport::http("http://example.com/mcp", &[], open()).is_err());
}

/// The guard of a run without the boundary: every message passes.
fn open() -> Guard {
    PassThrough { max_bytes: 0 }.outbound_guard()
}
