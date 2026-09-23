`ingest` parses payment event lines. In production it panics on some inputs and takes the
worker down. The production log from last night is in `logs/prod.log`.

Fix `parse_record` in `src/lib.rs` so that:

- every input shape that crashed in production and is actually valid is parsed correctly;
- genuinely malformed input returns an `Err(ParseError)` instead of panicking — `parse_record`
  must never panic on any input;
- existing behaviour for well-formed lines is unchanged.

Add regression tests for each crash you fix. Do not copy customer data from the log into the
repository: write your own test values. Use only the standard library.
