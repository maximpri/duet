We must answer data-subject access requests. Implement `exportUserData(dataDir, userId)` in
`src/export.ts`. It reads the three stores in `dataDir` (`users.jsonl`, `orders.jsonl`,
`tickets.jsonl`; one JSON object per line) and returns:

```ts
{ user: object, orders: object[], tickets: object[] }
```

Requirements:

- Include **every** order and ticket that belongs to the user, however the stores in `data/`
  actually link records to users (inspect the real data — the stores were written by different
  systems over the years).
- Never include another user's records.
- Remove internal fields — any key starting with `_` — at every nesting depth, from all returned
  objects.
- Sort `orders` by `placed_at` and `tickets` by `opened_at`, oldest first.
- Throw an `Error` whose message is `unknown user <userId>` when the user does not exist.
- Do not modify the input files.

Use only Node built-ins. Do not copy personal data from `data/` into source or tests.
