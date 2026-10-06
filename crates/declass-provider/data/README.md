# OpenRouter price snapshot

`openrouter-pricing.json` contains factual model IDs and pricing fields from the public,
unauthenticated `https://openrouter.ai/api/v1/models` endpoint, fetched on 2026-09-30 UTC.
`fetched_at` records the fetch time in Unix seconds. All other fields are retained as returned;
model descriptions and other catalog metadata are omitted.

Declass refreshes the public catalog at most once per day when starting a frontier session.
This snapshot is the fallback when no saved catalog is available. The displayed source and age
identify which prices were used. Token prices are USD per token in the source; the provider
module converts them to USD per million tokens for display and calculation. Cache rates and
conditional token-price overrides are retained.

Schema references:

- https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties
- https://openrouter.ai/docs/guides/best-practices/prompt-caching
- https://github.com/OpenRouterTeam/terraform-provider-openrouter/blob/main/docs/data-sources/model.md

The older `src/price.rs` table remains for reproducible frozen evaluation results. CLI runs,
sessions, delegated frontier work and optional security opinions use the catalog module.
