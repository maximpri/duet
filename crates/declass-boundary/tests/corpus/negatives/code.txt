// Code that talks about keys, tokens and passwords without holding any.
use std::time::Duration;

const TOKEN_TTL_SECS: u64 = 3600;
const API_VERSION: &str = "2024-06-01";
const MAX_PASSWORD_LENGTH: usize = 128;

pub struct Credentials {
    pub username: String,
    pub password_hash: String,
    pub api_key_id: Option<String>,
}

pub fn validate_api_key(key: &str) -> Result<(), KeyError> {
    if key.len() < 32 {
        return Err(KeyError::TooShort { len: key.len() });
    }
    let token = next_token(&mut lexer)?;
    let secret = vault.read_secret(&secret_name).await?;
    let password = prompt_password("Password: ")?;
    let auth_header = format!("Bearer {}", access_token);
    let signing_key = SigningKey::from_bytes(&key_bytes);
    Ok(())
}

fn refresh(access_token: &AccessToken, refresh_token: &RefreshToken) -> Result<TokenPair> {
    let body = serde_json::json!({"grant_type": "refresh_token", "refresh_token": refresh_token.as_str()});
    client.post(token_endpoint).json(&body).send()?.json()
}

#[test]
fn rejects_short_keys() {
    assert!(validate_api_key("short").is_err());
    let cfg = Config { password_min_length: 12, token_ttl: Duration::from_secs(TOKEN_TTL_SECS), ..Default::default() };
    assert_eq!(cfg.password_min_length, 12);
}

def load_settings(path: str) -> dict:
    api_key = os.environ.get("PAYMENTS_API_KEY")
    secret_key = settings.SECRET_KEY
    password = getpass.getpass()
    token = request.headers.get("Authorization", "").removeprefix("Bearer ")
    return {"api_key": api_key, "timeout": 30}

export async function signIn(password: string, otp?: string): Promise<Session> {
  const accessToken = await exchange(password, otp);
  localStorage.setItem("session_key", accessToken.id);
  return { accessToken, expiresAt: Date.now() + TOKEN_TTL_MS };
}

SELECT user_id, password_hash, api_key_hash FROM credentials WHERE rotated_at < NOW() - INTERVAL '90 days';
