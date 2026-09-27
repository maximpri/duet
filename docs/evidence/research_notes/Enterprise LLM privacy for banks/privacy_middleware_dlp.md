# Privacy middleware, AI gateways and DLP for LLMs (enterprise/bank market map, as of Sept 2026)

Scope note: research done 2026-09-25. The session's web-search budget ran out before Protecto funding, Lakera pricing, Azure AI Content Safety PII features and a few pricing pages could be checked; those are listed under Gaps. Vendor accuracy numbers are vendor claims unless marked "independent".

The market splits into four product shapes, and most of the named vendors now sit inside a larger security platform:
1. **Workforce / "shadow AI" DLP**: stops employees pasting data into ChatGPT, Claude, Gemini and similar tools. It runs through a SASE/SSE proxy, a browser extension or an endpoint agent. Vendors: Zscaler, Netskope, Palo Alto AI Access Security, Cisco AI Access, Microsoft Purview, Cloudflare One, Harmonic, Nightfall, Prompt Security/SentinelOne, Aim/Cato.
2. **Application runtime guardrails / AI firewalls**: an API or proxy in front of an in-house LLM app that checks prompts and responses for injection, toxicity and PII. Vendors: Lakera/Check Point, Prisma AIRS (Protect AI), CalypsoAI/F5, Pangea/CrowdStrike, Cisco AI Defense (Robust Intelligence), Lasso, Bedrock Guardrails, Google Model Armor, Cloudflare Firewall for AI.
3. **AI gateways with PII plugins**: Portkey, Kong AI Gateway, LiteLLM, Cloudflare AI Gateway.
4. **Privacy vaults / de-identification engines**: reversible tokenization, with values restored in the response. Vendors: Skyflow, Protecto, Private AI (now Limina), Tonic Textual, Protegrity. Open-source options: Presidio, LLM Guard, OpenAI Privacy Filter.

## 1. Vendors: how their technology works and what they claim

### Takeaway
Almost every product uses the same pipeline: detect entities (regex/checksum plus NER or transformer classifiers, now increasingly LLM or SLM classifiers), then block, one-way mask, or replace with a token/placeholder that is swapped back into the response.

The differences between vendors are mainly:
- **Where it runs:** browser, endpoint, SASE edge, gateway sidecar, or cloud API.
- **Reversibility:** whether the replacement can be undone.
- **Accuracy claims:** these are unverified and usually quoted at 95–99.9%.

Cloud-provider guardrails (Bedrock, Model Armor) only mask one way. Vault vendors (Skyflow, Protecto) and gateway plugins (Kong, LiteLLM, LLM Guard) offer reversible substitution.

### Cited Findings

**Privacy vaults / de-identification engines**
- **Skyflow LLM Privacy Vault (launched May 2023).**
  - De-identifies through tokenization or masking, plus a "sensitive data dictionary" for customer-defined terms.
  - Uses *deterministic* tokens (the same value always gives the same token). Tokens are swapped back to plaintext in LLM responses, only for authorized users.
  - Runs as SaaS vault infrastructure "available in over 100 countries" for data residency.
  - Sources: [Skyflow blog, 2023-05-18](https://www.skyflow.com/post/generative-ai-data-privacy-skyflow-llm-privacy-vault); [VentureBeat](https://venturebeat.com/ai/skyflow-launches-privacy-vault-for-building-llms)
- **Skyflow (later additions).**
  - Now markets an "AI Gateway" with "two-way data rehydration" for agents.
  - Claims "polymorphic encryption" and tokenization that keep referential integrity. This comes from a third-party summary, not a primary spec.
  - Sources: [WorkOS comparison](https://workos.com/blog/skyflow-vs-workos-agentic-security); [Skyflow tokenization page](https://www.skyflow.com/tokenization)
- **Protecto.**
  - Detection uses proprietary models plus "DeepSight", with "industry-specific add-ons for healthcare and banking". Covers "200+ sensitive data types".
  - Tokens are deterministic ("same input always produces the same token within a namespace"), format-preserving, "entropy-based" and reversible for authorized users.
  - Claims >99% detection on the homepage and 99.9% in case studies. Also claims "accuracy parity or better" for downstream AI, measured with its own "RARI" (Reasoning & Retrieval Integrity) metric.
  - Deploys as SaaS, hosted VPC, or air-gapped on-prem.
  - Source: [Protecto homepage](https://www.protecto.ai/)
- **Protecto for banks.** Markets "Sovereign AI for Banks": DeepSight + GPTGuard deployed inside a bank's private cloud as a sovereign AI gateway, with data "kept inside the country". Sources: [Protecto banks page](https://www.protecto.ai/industry/sovereign-ai-for-banks/); [Protecto MEA page](https://www.protecto.ai/mea/)
- **Private AI renamed itself Limina on 2026-03-05.** It was a rebrand, not an acquisition, and "does not affect Limina's products, customers, or operations". It handles PII/PHI in text, images, audio and documents. Target sectors include financial services and insurance. Source: [Limina rebrand blog](https://www.getlimina.ai/en/blog/private-ai-rebrands-limina-sensitive-data-privacy)
- **Private AI benchmarking method.** The company published a PII benchmarking methodology that compares against AWS, Google and Azure only on entity types both services support. Source: [Private AI blog](https://www.private-ai.com/en/blog/pii-dectection-benchmark)
- **Tonic Textual.** A de-identification vendor that published a benchmark putting itself at F1 0.96 (see Section 3). Source: [Tonic benchmark](https://www.tonic.ai/ai-model-benchmarks/textual-benchmark)
- **Protegrity.** A long-standing bank tokenization vendor. It markets a "Protegrity-PII" model positioned against OpenAI's Privacy Filter. Source: [Protegrity blog](https://www.protegrity.com/blog/open-ai-privacy-filter-protegrity-pii-and-the-data-lesson/)

**Workforce DLP / shadow-AI**
- **Nightfall AI.**
  - Says it "trains transformer-based detectors on labeled sensitive-data examples rather than relying on regular expressions", using "ML detectors and LLM classifiers".
  - Covers PII, PHI, PCI, API keys, secrets, passwords and source code. Claims "95% precision".
  - Covers SaaS, email, AI apps, endpoints and browsers. The Premier edition adds Cursor, Claude Code, VS Code, MCP servers and Claude Enterprise.
  - Source: [Nightfall pricing page](https://www.nightfall.ai/pricing)
- **Nightfall "Firewall for AI"** is a client wrapper around GenAI API calls that detects and removes PII, PCI, PHI and secrets in real time. Source: [Nightfall Firewall for AI](https://www.nightfall.ai/lp/firewall-for-ai)
- **Harmonic Security.**
  - Product is a lightweight browser extension ("Harmonic Protect") covering "ChatGPT, Gemini, Perplexity and 1,000+ AI surfaces".
  - Uses "purpose built small language models" and claims "96% greater accuracy than legacy DLP" for source code, M&A data and PII.
  - Customers define sensitive data in natural language rather than regex policies.
  - Sources: [Harmonic browser solution](https://www.harmonic.security/solutions/browser-based-genai-security); [Harmonic DLP for GenAI](https://www.harmonic.security/solutions/dlp-for-genai); [Expert Insights](https://expertinsights.com/ai-solutions/the-top-genai-security-solutions)
- **Harmonic's models (AWS case study, 2025-12-11).**
  - Detection runs on fine-tuned ModernBERT-base (149M parameters, binary M&A classifier) and ModernBERT-large (395M, multi-label), which replaced an 8B baseline model.
  - Median latency fell to 46 ms (from 189 ms). Target was <500 ms at p95. Accuracy rose 1.56% and F1 rose 2.26%.
  - Training data was synthetic, generated with Llama 3.3 70B and Amazon Nova Pro.
  - Models run in the cloud on SageMaker ml.g5.4xlarge, not in the browser.
  - Source: [AWS ML blog](https://aws.amazon.com/blogs/machine-learning/how-harmonic-security-improved-their-data-leakage-detection-system-with-low-latency-fine-tuned-models-using-amazon-sagemaker-amazon-bedrock-and-amazon-nova-pro/)
- **Prompt Security (now SentinelOne).**
  - Uses an agent and browser extensions to find sanctioned and shadow AI across browsers, desktop IDEs, terminal assistants and APIs.
  - Policy rules can "redact or tokenize sensitive data on the fly", block risky prompts, or coach users inline. Redaction is edge-based and covers PII, source code and IP.
  - Covers GitHub Copilot, Cursor and Claude Code "without exposing IP or secrets".
  - Sources: [SentinelOne Prompt Security page](https://www.sentinelone.com/platform/securing-ai-prompt/); [AppSecSanta review](https://appsecsanta.com/prompt-security)
- **Zscaler (AI Guard / Zero Trust Exchange).**
  - Inspects prompts and responses inline, and can block or redact SSNs, card numbers and source code.
  - Can render AI apps in Browser Isolation with clipboard and upload restrictions.
  - Sources: [Zscaler GenAI page](https://www.zscaler.com/products-and-solutions/securing-generative-ai); [Zscaler blog](https://www.zscaler.com/blogs/product-insights/make-generative-ai-tools-chatgpt-safe-and-secure-zscaler)
- **Zscaler scale claim.** A search snippet states "ChatGPT alone generated 410 million DLP policy violations in a single year". This was not verified against the primary ThreatLabz report. Source: [Zscaler blog](https://www.zscaler.com/blogs/product-insights/ai-prompt-data-leakage-examples)
- **Netskope.**
  - Netskope One DLP and "Skylight AI Guardrails" inspect every prompt and response for source code, PII and IP.
  - The design favours real-time "user coaching" pop-ups before hard blocks.
  - Uses "semantic inspection" because regex fails when AI rewrites content.
  - Sources: [Netskope Securing GenAI](https://www.netskope.com/products/securing-generative-ai); [Netskope community](https://community.netskope.com/inside-netskope-22/securing-saas-and-genai-without-saying-no-8705); [SkopeAI brief](https://www.netskope.com/resources/solution-briefs/skopeai-for-chatgpt-and-generative-ai)
- **Palo Alto Networks.**
  - *AI Access Security* (SASE) inspects prompts and attachments inline before they reach external AI.
  - *Prisma AIRS* runtime security claims ">1,000 predefined data patterns (regex and ML-based)". It masks only the sensitive spans, with high/medium/low confidence, and deploys as Network Intercept or API Intercept.
  - Prisma AIRS 2.0 completed the Protect AI integration. Prisma AIRS 3.0 (2026) targets agentic AI.
  - Sources: [Prisma AIRS API use cases](https://pan.dev/prisma-airs/api/airuntimesecurity/usecases/); [AI Access Security](https://www.paloaltonetworks.com/sase/ai-access-security); [AIRS 3.0 press](https://www.paloaltonetworks.com/company/press/2026/palo-alto-networks-secures-agentic-ai-with-prisma-airs-3-0)
- **Cisco AI Defense** (built on Robust Intelligence) has four components: AI Access, AI Cloud Visibility, Model & App Validation, and Runtime Protection. AI Access enforces DLP inside Cisco Secure Access SSE and uses guardrails that "infer intent". Sources: [Cisco AI Defense](https://www.cisco.com/site/us/en/products/security/ai-defense/index.html); [AI Access](https://www.cisco.com/site/us/en/products/security/ai-defense/ai-access/index.html)
- **Microsoft Purview (DSPM for AI + DLP).**
  - Detects common sensitive info types inline and blocks prompts to third-party AI sites in Edge. Endpoint DLP on onboarded Windows machines can warn or block pastes into ChatGPT.
  - DLP for Microsoft 365 Copilot prompts went GA around Ignite 2025. If a sensitive type is detected, "no AI response is generated, and no data is sent for Graph or web grounding".
  - This is block-only, not redact-and-forward.
  - Sources: [Microsoft Learn: DLP for M365 Copilot](https://learn.microsoft.com/en-us/purview/dlp-microsoft365-copilot-location-learn-about); [Tech Community](https://techcommunity.microsoft.com/blog/microsoft-security-blog/safeguarding-sensitive-data-in-microsoft-365-copilot-interactions-dlp-for-micros/4512497); [DSPM for AI considerations](https://learn.microsoft.com/en-us/purview/dspm-for-ai-considerations)
- **Cloudflare One "AI prompt protection"** (open beta, 2025-08-25).
  - Classifies prompts and responses to ChatGPT, Gemini, Claude and Perplexity.
  - Runs several models in parallel on its own Workers AI: **Presidio** (PII), **PromptGuard2** (jailbreak), **Llama3-70B** (topic detection) and bge-m3 embeddings. It says "user prompts are not sent to third-party vendors".
  - Detects PII, credentials/secrets, source code and financial info.
  - Logs are encrypted with a customer-held public key.
  - Source: [Cloudflare blog](https://blog.cloudflare.com/ai-prompt-protection/)

**Runtime guardrails / AI firewalls for in-house apps**
- **Amazon Bedrock Guardrails sensitive-information filter.**
  - AWS calls it a "probabilistic machine learning (ML) based solution that is context-dependent", plus custom regex (no lookaround).
  - Actions are BLOCK, ANONYMIZE or NONE. Anonymize replaces the entity with its type, e.g. `{NAME}`, `{EMAIL}`. It is **one-way**, with no restore.
  - Covers ~30 built-in types, including CREDIT_DEBIT_CARD_*, PIN, IBAN, SWIFT_CODE, US bank account/routing numbers, AWS keys and PASSWORD. Works on code (variable names, hard-coded credentials).
  - Documented gaps:
    - It does **not** check tool-call arguments, tool results or tool definitions.
    - Invocation logs keep the **original unmasked** input.
    - The trace `match` field returns the raw PII.
    - It needs "sufficient context", so short strings are detected poorly.
  - Source: [AWS docs](https://docs.aws.amazon.com/bedrock/latest/userguide/guardrails-sensitive-filters.html)
- **Google Cloud Model Armor + Sensitive Data Protection (SDP).**
  - Basic mode covers card numbers, SSN, financial account numbers, ITIN, GCP credentials and API keys.
  - Advanced mode uses SDP inspect + de-identify templates that "transform, tokenize, and redact" with 150+ infoTypes. The default replaces a value with its infoType name.
  - Sources: [Google Model Armor](https://cloud.google.com/security/products/model-armor); [Google Cloud community (Medium)](https://medium.com/google-cloud/secure-ai-applications-with-model-armor-and-sensitive-data-protection-sdp-8451e54a52f9); [CamoText comparison](https://camotext.ai/blogposts/best-text-anonymization-tools-comparison.html)
- **Cloudflare Firewall for AI** (WAF, now called "AI Security for Apps").
  - Detects PII in incoming prompts (phones, emails, SSNs, card numbers), unsafe topics and prompt injection.
  - It is an Enterprise-only paid add-on. It works only on JSON requests to endpoints labelled `cf-llm`, and mitigates through WAF rules.
  - Source: [Cloudflare docs](https://developers.cloudflare.com/waf/detections/firewall-for-ai/)
- **Lakera Guard (now Check Point).** Screens prompts and responses in real time for injection, jailbreaks, data leakage and sensitive content, using proprietary models. Sources: [Fortune, 2024-07-24](https://fortune.com/2024/07/24/lakera-20-million-funding-ai-chatbot-security); [Check Point press](https://www.checkpoint.com/press-releases/check-point-acquires-lakera-to-deliver-end-to-end-ai-security-for-enterprises/)
- **CalypsoAI (now F5 AI Guardrails / AI Red Team).**
  - Scanners "combine NER and classification models" for PII.
  - Being folded into F5's Application Delivery and Security Platform.
  - Sources: [Maxim AI roundup](https://www.getmaxim.ai/articles/top-5-genai-guardrails-platforms-for-financial-compliance/) (secondary); [F5 blog](https://www.f5.com/company/blog/what-are-ai-guardrails)
- **Pangea (now CrowdStrike AIDR).** Offered "AI Guard" (sensitive data leakage) and "Prompt Guard" (jailbreak), which now form CrowdStrike Falcon's "AI Detection and Response". Source: [SecurityWeek](https://www.securityweek.com/crowdstrike-to-acquire-pangea-to-launch-ai-detection-and-response-aidr/)
- **Lasso Security.** Independent as of mid-2026 and named a Representative Vendor in Gartner's 2025 AI TRiSM Market Guide (runtime inspection & enforcement). Sources: [Lasso blog](https://www.lasso.security/blog/gartner-names-lasso-security-as-a-representative-vendor); [The Hacker News, July 2026](https://thehackernews.com/expert-insights/2026/07/a-look-inside-lassos-ai-security.html)

**AI gateways and open-source**
- **Portkey.** PII redaction runs "across 5 guardrail providers" and covers phone, email, location, IP, SSN, names, cards and "confidential information". Sources: [Portkey docs](https://portkey.ai/docs/product/guardrails/pii-redaction); [TrueFoundry pricing guide](https://www.truefoundry.com/blog/portkey-pricing-guide)
- **Kong AI Gateway: AI PII Sanitizer plugin.**
  - Detection runs in Kong's own "AI PII Anonymizer Service" container, a self-hosted sidecar needing ≥600 MB RAM.
  - Replacement modes:
    - *Placeholder*: `PLACEHOLDER{i}`, with the same value always getting the same placeholder.
    - *Synthetic*: the value is swapped for a same-type surrogate, e.g. "John" becomes "Amir".
  - Optional restoration puts the original values back into responses.
  - Covers credit cards, crypto addresses, bank accounts, national IDs and custom regex.
  - Available in Enterprise only.
  - Source: [Kong docs](https://developer.konghq.com/plugins/ai-sanitizer/)
- **LiteLLM (OSS proxy).**
  - The Presidio guardrail runs in modes `pre_call`, `post_call`, `logging_only` and `pre_mcp_call`, with actions MASK (e.g. `<CREDIT_CARD>`) or BLOCK, plus per-entity score thresholds.
  - `output_parse_pii` swaps masked tokens in the LLM response back to the original values.
  - Needs Presidio containers. The request body limit defaults to 500 KB.
  - Source: [LiteLLM docs](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2)
- **Presidio (open source, MIT).**
  - **Moved from Microsoft to the community-run "Data Privacy Stack" organization in 2026.** Microsoft stays on the steering committee, and images moved to ghcr.io/data-privacy-stack.
  - Detection combines spaCy/Stanza/Transformers NER, regex, checksums, context words, and external models (Flair, GLiNER, Azure AI Language).
  - Operators include encrypt/decrypt and pseudonymization with mappings.
  - Official disclaimer: "there is no guarantee that Presidio will find all sensitive information".
  - Sources: [Presidio site](https://presidio.dataprivacystack.org/); [transition note](https://presidio.dataprivacystack.org/project_transition/); [GitHub](https://github.com/data-privacy-stack/presidio)
- **LLM Guard (Protect AI OSS).**
  - The Anonymize scanner uses Presidio plus BERT/DeBERTa NER (dslim/bert-base-NER by default; AI4Privacy DeBERTa variants optional).
  - It can use `[REDACTED_...]` placeholders or Faker synthetic values. The Vault plus the Deanonymize output scanner restore the originals.
  - Docs note that entity detection is "English-specific".
  - Source: [LLM Guard docs](https://protectai.github.io/llm-guard/input_scanners/anonymize/)
- **OpenAI Privacy Filter (released 2026-04-22, Apache 2.0).**
  - A bidirectional token classifier: 1.5B total / 50M active parameters (sparse MoE, 8 blocks), 128K context, runs locally.
  - Covers 8 span types: account numbers, addresses, emails, names, phones, URLs, dates, and **secrets** (credentials/API keys).
  - Reported 97.43% F1 on PII-Masking-300k.
  - Model-card limitations: misses uncommon names, over-redacts public entities, weaker on non-English and non-Latin scripts, and is "not a blanket anonymization claim".
  - Sources: [HF model card](https://huggingface.co/openai/privacy-filter); [Help Net Security](https://www.helpnetsecurity.com/2026/04/23/openai-privacy-filter-personally-identifiable-information/); [aimadetools guide](https://www.aimadetools.com/blog/openai-privacy-filter-guide/)

### Inferences
- **No cloud-native restore.** Cloud-native guardrails (Bedrock, Model Armor basic, Purview for Copilot) are one-way or block-only. A bank that wants the answer to contain the real account or customer name must add a vault or gateway layer (Skyflow, Protecto, Kong, LiteLLM `output_parse_pii`, LLM Guard Vault) or build its own mapping.
- **The workforce-DLP layer is consolidating.** It is being absorbed into SASE/SSE suites (Zscaler, Netskope, Palo Alto, Cisco, Cato, Cloudflare). The main standalone players remaining are Harmonic, Nightfall and Lasso.
- **Agentic traffic is a known blind spot.** Bedrock's documented tool-call gap shows that agent traffic (tool arguments and results, MCP) was not covered by first-generation PII filters. Vendors are now marketing MCP/agent coverage (Nightfall Premier, LiteLLM `pre_mcp_call`, Prisma AIRS 3.0).

### Gaps
- **Azure AI Content Safety:** I could not confirm whether it offers a PII redaction filter separate from Azure AI Language PII detection. Only Prompt Shields (jailbreak) was confirmed.
- **Skyflow:** its current detection engine (own NER or LLM) is not documented in the sources reached.
- **Lakera Guard:** the PII detection method (regex vs model) and entity list were not documented in the sources reached.
- **Aim Security:** product details after the Cato acquisition were not fetched.

## 2. Detection techniques; reversible pseudonymization vs one-way masking; source code and secrets

### Takeaway
Detection has moved through three generations:
1. Regex/checksum (still best for structured IDs such as emails, card numbers and IBANs).
2. NER models (spaCy, BERT/DeBERTa, GLiNER, used by Presidio and LLM Guard).
3. Fine-tuned transformer/SLM classifiers or LLM topic classifiers (Nightfall, Harmonic ModernBERT, Cloudflare Llama3-70B, OpenAI Privacy Filter). These catch *semantic* categories such as M&A, source code and strategy that regex cannot.

Reversibility is the main architectural choice:
- **One-way type-tag masking** is safest, but it degrades answers.
- **Reversible deterministic tokens or surrogates** keep entity identity and let the vault restore values in the response.

Secrets are handled mostly by pattern and entropy detectors, which have high recall but low precision.

### Cited Findings
- **Regex vs NER on structured IDs.** Presidio's pattern recognizer beats every NER model on email (F1 0.930–0.996). Adding NER on top of regex added only +0.001–0.012 F1 in one 2026 benchmark. Source: [Sikkema benchmark, 2026-06-01](https://albertsikkema.com/python/security/privacy/2026/06/01/benchmarking-open-source-pii-detection.html)
- **Bedrock Guardrails:** ML-based and "context-dependent", plus regex. It needs context to disambiguate digit strings (a KMS key vs a user ID). Source: [AWS docs](https://docs.aws.amazon.com/bedrock/latest/userguide/guardrails-sensitive-filters.html)
- **Semantic categories:**
  - Nightfall uses transformer detectors plus "LLM classifiers". Source: [Nightfall](https://www.nightfall.ai/pricing)
  - Harmonic uses fine-tuned ModernBERT classifiers for categories such as M&A. Source: [AWS blog](https://aws.amazon.com/blogs/machine-learning/how-harmonic-security-improved-their-data-leakage-detection-system-with-low-latency-fine-tuned-models-using-amazon-sagemaker-amazon-bedrock-and-amazon-nova-pro/)
  - Cloudflare uses Llama3-70B topic classification plus Presidio. Source: [Cloudflare](https://blog.cloudflare.com/ai-prompt-protection/)
  - Netskope argues regex fails when AI "transforms or rewrites content while preserving its meaning". Source: [Netskope](https://www.netskope.com/products/securing-generative-ai)
- **Replacement styles in use:**
  - *Type tag* (one-way): Bedrock `{NAME}`, LiteLLM `<CREDIT_CARD>`, Model Armor infoType name. Sources: [AWS](https://docs.aws.amazon.com/bedrock/latest/userguide/guardrails-sensitive-filters.html); [LiteLLM](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2)
  - *Indexed placeholder* (reversible): Kong `PLACEHOLDER{i}`, LLM Guard `[REDACTED_CUSTOM_1]` with Vault. Sources: [Kong](https://developer.konghq.com/plugins/ai-sanitizer/); [LLM Guard](https://protectai.github.io/llm-guard/input_scanners/anonymize/)
  - *Synthetic surrogate* (reversible): Kong synthetic mode, LLM Guard `use_faker`. Same sources as above.
  - *Deterministic format-preserving vault tokens* (reversible, access-controlled): Skyflow, Protecto. Sources: [Skyflow](https://www.skyflow.com/post/generative-ai-data-privacy-skyflow-llm-privacy-vault); [Protecto](https://www.protecto.ai/)
  - *Encryption operator* (reversible): Presidio encrypt/decrypt. Source: [Presidio](https://presidio.dataprivacystack.org/)
- **Restoration can fail.** In an open-source benchmark, redact-then-restore round-trip pass rates were 100% for regex, Piiranha and GLiNER but **64% for Presidio**, because of span-boundary misalignment. Source: [Sikkema benchmark](https://albertsikkema.com/python/security/privacy/2026/06/01/benchmarking-open-source-pii-detection.html)
- **Source code and secrets:**
  - Bedrock claims PII detection in "code syntax, comments, string literals" and hard-coded credentials, with AWS_ACCESS_KEY/AWS_SECRET_KEY/PASSWORD types. Source: [AWS](https://docs.aws.amazon.com/bedrock/latest/userguide/guardrails-sensitive-filters.html)
  - OpenAI Privacy Filter includes a "secrets" class. Source: [HF](https://huggingface.co/openai/privacy-filter)
  - Prompt Security and Nightfall target coding agents (Copilot, Cursor, Claude Code). Sources: [SentinelOne](https://www.sentinelone.com/platform/securing-ai-prompt/); [Nightfall](https://www.nightfall.ai/pricing)
  - Zscaler, Netskope and Cloudflare classify "source code" as a category, usually to block rather than redact. Sources: [Zscaler](https://www.zscaler.com/products-and-solutions/securing-generative-ai); [Cloudflare](https://blog.cloudflare.com/ai-prompt-protection/)
- **Secret-scanner accuracy** (independent academic study):
  - Precision: GitHub Secret Scanner 75%, Gitleaks 46%.
  - Recall: Gitleaks 88%, SpectralOps 67%, TruffleHog 52%.
  - False positives come from generic regex and entropy heuristics. False negatives come from faulty regex and skipped file types.
  - Sources: [arXiv 2307.00714](https://arxiv.org/pdf/2307.00714); [GitGuardian blog](https://blog.gitguardian.com/secrets-detection-accuracy-precision-recall-explained/)
- **What employees actually paste** (Harmonic telemetry):
  - Q2 2025: 4.37% of prompts and ~22% of uploaded files contained sensitive content, across 1M prompts and 20K files in 300+ apps. Categories included source code, access credentials, M&A documents, customer and employee records, and internal financials. Files accounted for 79.7% of credit-card exposures. Source: [BusinessWire via Morningstar, 2025-07-31](https://www.morningstar.com/news/business-wire/20250731456105/22-of-all-files-and-437-of-prompts-submitted-to-genai-tools-by-employees-contain-sensitive-data)
  - An earlier report said "nearly 10%" of prompts contained sensitive data. Source: [CSO Online](https://www.csoonline.com/article/3819170/nearly-10-of-employee-gen-ai-prompts-include-sensitive-data.html)

### Inferences
- For source code, entity-level redaction mostly doesn't fit. Code *is* the IP, so products either block it, coach the user, or redact only the secrets inside it. None of the reviewed vendors claims to usefully pseudonymize proprietary code identifiers while keeping it useful to a frontier model. LLM-Redactor's 31.3% residual leak on proprietary code (Section 3) supports this.
- Reversible substitution means the vault or mapping itself becomes sensitive data. Deterministic tokens also enable linkage across sessions, which is intended for audit (Protecto says so explicitly) but increases re-identification risk.

### Gaps
- No vendor-independent measurement was found of LLM-classifier false-positive rates on semantic categories such as M&A or trading strategy.

## 3. Measured accuracy, independent evaluations and failure modes

### Takeaway
Vendor claims (95–99.9%) are not reproduced by independent tests.

- Independent 2026 benchmarks put open-source detectors at F1 ≈0.48–0.54 averaged across domains.
- Presidio scored F1 0.086 on context-aware multi-turn PII.
- Cross-domain F1 can fall to ~0.14.

Frontier LLMs and new small specialist models (OpenAI Privacy Filter, a 0.6B PII-Tracer) do better but still miss a lot. Long context is a known weakness.

Masking also hurts answer quality, most of all for retrieval tasks and the strongest models; reversible pseudonyms hurt much less than redaction. Beyond detection misses, the documented failure modes are:
- LLMs inferring attributes from quasi-identifiers.
- Guardrail evasion through character injection.
- Logs, traces and tool calls that bypass masking.

### Cited Findings
- **PII-TRACE (arXiv, 2026-08-31)**, context-aware PII in multi-turn chats, character-level F1:
  - PII-Tracer 0.6B: 0.629
  - GPT-5.6-sol: 0.611
  - OpenAI Privacy Filter: 0.492
  - **Presidio: 0.086**
  - Specialized public detectors showed "high recall but low precision"; frontier LLMs were more precise.
  - Recall fell from 0.975 (<1K chars) to 0.687 (>10K chars).
  - "The same text span can be PII in one conversation but not in another."
  - Source: [arXiv 2609.22200](https://arxiv.org/html/2609.22200)
- **Independent open-source benchmark (2026-06-01)**, 4 datasets including Gretel Finance, average F1:
  - Piiranha 0.542, GLiNER v1 0.535, Presidio 0.481, GLiNER v2 0.478, regex 0.171. The author's verdict: "none of these models are good".
  - Piiranha dropped 78% out of distribution (0.780 → 0.169 on financial text).
  - Names in financial text were detected with F1 only 0.139 by Piiranha.
  - Commercial claims of 0.92–0.99 "drop to 0.14–0.65 on cross-domain tests". PIIBench's best system scored F1 0.14 across 10 unified datasets.
  - CPU speed per text: Presidio 15 ms, GLiNER ~160–200 ms.
  - Sources: [Sikkema](https://albertsikkema.com/python/security/privacy/2026/06/01/benchmarking-open-source-pii-detection.html); [PIIBench arXiv 2604.15776](https://arxiv.org/pdf/2604.15776)
- **Vendor-run benchmark (Tonic, Oct 2025)**, word-level F1 on legal, EHR and call-transcript data:
  - Tonic Textual 0.96, AWS Comprehend 0.88, spaCy 0.64, Azure AI Language 0.61, Google DLP 0.61, Presidio 0.57, GLiNER 0.32.
  - This is a vendor benchmark, so treat it as indicative only.
  - Source: [Tonic](https://www.tonic.ai/ai-model-benchmarks/textual-benchmark)
- **Other Presidio results:**
  - Precision 0.23–0.25 on math-tutoring PII. Source: [arXiv 2602.16571](https://arxiv.org/pdf/2602.16571)
  - Passport-number F1 0.33 vs 0.95 for IBM's OneShield. Source: [arXiv 2501.12465](https://arxiv.org/pdf/2501.12465)
- **Utility loss from anonymization (arXiv 2609.11335, Sept 2026; Bonn/Fraunhofer/Microsoft Germany):**
  - Across 5 LLMs and 11 benchmarks, "more capable models suffer the largest performance drops". GPT-4o mini on the RGB retrieval benchmark fell **0.80 → 0.32**.
  - Reversible, uniqueness-preserving anonymization "substantially outperformed" irreversible redaction.
  - Telling the model that the text had been anonymized gave no benefit.
  - Sources: [arXiv](https://arxiv.org/abs/2609.11335); [PPC Land summary](https://ppc.land/anonymizing-prompts-cuts-openais-gpt-4o-mini-retrieval-score-by-60/)
- **Task-dependent degradation.** Sentiment tasks are robust, while ANLI, MedNLI and fake-news detection degrade significantly. Source: [Tau-Eval arXiv 2506.05979](https://arxiv.org/pdf/2506.05979). A contrary result: Redakto reports utility "on par with original texts" for its redaction strategies. Source: [arXiv 2608.18260](https://arxiv.org/abs/2608.18260v1)
- **LLM-Redactor (arXiv 2604.12064, 2026-04-13)** evaluated 8 techniques: local-only, redaction + placeholder restore, semantic rephrasing, TEE inference, split inference, FHE, MPC, and DP noise.
  - Test set: 1,300 samples with 4,014 annotations.
  - The best combination (local + redaction + rephrasing) reached 0.6% combined PII leak, with zero exact PII leaks in 500 samples, but **31.3% leak on proprietary code**.
  - "No single technique universally dominates."
  - Source: [arXiv](https://arxiv.org/abs/2604.12064)
- **Re-identification by inference.**
  - GPT-4 inferred personal attributes (location, income, sex) from Reddit text with 85.5% top-1 and 95.2% top-3 accuracy.
  - "Text anonymization and model alignment are currently ineffective" against this (ICLR 2024).
  - Sources: [arXiv 2310.07298](https://arxiv.org/abs/2310.07298); [ETH SRI Lab](https://www.sri.inf.ethz.ch/publications/staab2023beyond)
- **Guardrail evasion.**
  - Character injection (emoji/Unicode variation-selector smuggling, bidirectional text) "fully bypassed" several guardrails including Protect AI v2 and Azure Prompt Shield.
  - Azure Prompt Shield average attack success: 71.98% for injection, 60.15% for jailbreak.
  - Six guardrail systems were tested, and all were vulnerable. The paper targets injection/jailbreak classifiers; the same tokenizer-stripping trick is plausibly relevant to PII detectors but untested there.
  - Source: [arXiv 2504.11168](https://arxiv.org/abs/2504.11168)
- **Bypass paths inside a product.** In Bedrock, PII masking does not apply to tool-call arguments or results, to CloudWatch/S3 invocation logs (which store the *original* input), or to the trace `match` field. Source: [AWS docs](https://docs.aws.amazon.com/bedrock/latest/userguide/guardrails-sensitive-filters.html)
- **Model-card admissions.** OpenAI Privacy Filter under-detects uncommon names and over-redacts public entities. Source: [HF](https://huggingface.co/openai/privacy-filter). Presidio: "no guarantee that Presidio will find all sensitive information". Source: [Presidio](https://presidio.dataprivacystack.org/)
- **Demographic bias** in name detection for PII masking is documented. Source: [arXiv 2205.04505](https://arxiv.org/pdf/2205.04505)

### Inferences
- For a bank, the realistic assumption is that entity-level detectors will miss a meaningful share of sensitive spans in long, domain-specific, multi-turn text. Semantic confidential material (strategy, MNPI, deal terms) is largely outside what entity detectors can see.
- The trade-off runs both ways. Aggressive one-way masking protects best but damages retrieval-heavy tasks (RAG over client documents) the most. Reversible surrogates keep utility but need a trusted mapping store and leave inference and linkage risk.

### Gaps
- No independent, published recall/precision evaluation was found for any *commercial* LLM-DLP product (Nightfall, Harmonic, Protecto, Skyflow, Lakera, Prisma AIRS) on bank-style data. All numbers are self-reported.
- No Gartner or Forrester accuracy comparison was accessible.

## 4. Does any product use a local/small model to do the sensitive processing while the frontier model reasons on redacted data?

### Takeaway
In shipping products, local or small models are used almost only for **detection and classification**, not for doing the sensitive part of the task:
- Harmonic's ModernBERT models.
- Cloudflare's self-hosted Presidio and Llama3-70B.
- OpenAI Privacy Filter, which runs locally.
- Kong's sidecar.

The "local model handles the private parts, frontier model reasons on a sanitized query, local model recombines" pattern exists in **research**: PAPILLON (NAACL 2025), LLM-Redactor, and 2026 follow-ups on contextual-integrity rewriting. No commercial vendor was found selling it as a product.

### Cited Findings
- **PAPILLON (NAACL 2025).**
  - A local model (Llama-3.1-8B) rewrites the user query into a privacy-preserving prompt for an API model (GPT-4o-mini), then an "Information Aggregator" combines the API answer with the private original locally.
  - Result: quality preserved 85.5% of the time with a 7.5% leak rate on the PUPA benchmark.
  - DSPy ships an RL tutorial for it.
  - Sources: [arXiv 2410.17127](https://arxiv.org/abs/2410.17127); [ACL Anthology](https://aclanthology.org/2025.naacl-long.173/); [DSPy tutorial](https://dspy.ai/tutorials/rl_papillon/)
- **LLM-Redactor.** Recommends combining local processing "when feasible" with redaction and rephrasing. The best combination leaked 0.6% on PII but 31.3% on proprietary code. Source: [arXiv 2604.12064](https://arxiv.org/abs/2604.12064)
- **2026 follow-ups on privacy-conscious delegation** (titles confirmed; contents not reviewed):
  - "Need to Know: Contextual-Integrity-Grounded Query Rewriting for Privacy-Conscious LLM Delegation". Source: [arXiv 2606.04067](https://arxiv.org/pdf/2606.04067)
  - "Beyond Direct Identifiers: Probabilistic Privacy Risk Estimation for Privacy-Conscious LLM Query Delegation". Source: [arXiv 2608.09140](https://arxiv.org/pdf/2608.09140)
  - "SurrogateShield: Beyond Redaction for High-Utility, Privacy-Preserving LLM Interactions". Source: [arXiv 2606.29567](https://arxiv.org/pdf/2606.29567)
- **Local SLM plus confidential VM** for prompt privacy: "Confidential Prompting / OSMD". Source: [arXiv 2409.19134](https://arxiv.org/html/2409.19134v2)
- **Commercial detection-only local or self-hosted models:**
  - OpenAI Privacy Filter runs locally, "without leaving your machine". Source: [aimadetools](https://www.aimadetools.com/blog/openai-privacy-filter-guide/)
  - Cloudflare runs Presidio and Llama3-70B on its own network so prompts "are not sent to third-party vendors". Source: [Cloudflare](https://blog.cloudflare.com/ai-prompt-protection/)
  - Kong's detection runs in a self-hosted container. Source: [Kong](https://developer.konghq.com/plugins/ai-sanitizer/)
  - Protecto offers air-gapped on-prem. Source: [Protecto](https://www.protecto.ai/)
  - Harmonic's SLMs run in AWS SageMaker, not on-device. Source: [AWS blog](https://aws.amazon.com/blogs/machine-learning/how-harmonic-security-improved-their-data-leakage-detection-system-with-low-latency-fine-tuned-models-using-amazon-sagemaker-amazon-bedrock-and-amazon-nova-pro/)
- **Closest shipping analogue: reversible substitution.** Skyflow, Protecto, Kong, LiteLLM and LLM Guard substitute locally and restore locally, so the frontier model only ever sees tokens. The local component is a rules/NER engine, not a reasoning model. Sources: [Skyflow](https://www.skyflow.com/post/generative-ai-data-privacy-skyflow-llm-privacy-vault); [LiteLLM](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2)

### Inferences
- There is a clear product gap. Nobody sells "a local LLM does the sensitive sub-task (e.g. reads the client file, computes on the raw figures) while a frontier model plans or reasons on the abstracted problem" for banks. The evidence that it can work is academic: PAPILLON's 85.5% quality / 7.5% leak.
- **Implication for Duet (not a finding):** a privacy-first local/frontier split would be different from today's DLP vendors, which only detect and substitute. It would still have to beat reversible-token gateways on utility, where the 2609.11335 results show reversible methods already recover much of the loss.

### Gaps
- Commercial offerings may exist that were not surfaced before the search budget ran out, for example confidential-computing vendors or "private LLM routing" startups.

## 5. Pricing, deployment modes and financial-services customers

### Takeaway
Public pricing is rare.

| Product | Price model |
|---|---|
| Bedrock Guardrails | Per text unit |
| Portkey | Tiered SaaS |
| Nightfall, Protecto, Skyflow, Limina, SASE vendors | Per user/year or enterprise quote |

Deployment matches the product shape:
- SASE proxy or browser extension or endpoint agent for workforce DLP.
- API or proxy for app guardrails.
- Self-hosted container or VPC or air-gapped for vaults and gateways.

Named financial-services customers are thin:
- Citi (investor in and customer of Lakera).
- Bank of Muscat (Protecto).
- Unnamed neo-banks and card platforms (Skyflow).
- An unnamed regional investment bank (endpoint DLP).

### Cited Findings
- **Amazon Bedrock Guardrails:** PII filter costs **$0.10 per 1,000 text units** (1 unit = up to 1,000 characters). Content and denied-topic filters were cut 80–85% to $0.15 per 1,000 units in Dec 2024. Sources: [AWS What's New, Dec 2024](https://aws.amazon.com/about-aws/whats-new/2024/12/amazon-bedrock-guardrails-reduces-pricing-85-percent/); [AWS pricing](https://aws.amazon.com/bedrock/pricing/)
- **Portkey:**
  - Tiers: free (10K requests/month), Pro $79/mo, Team $799/mo, Enterprise by quote.
  - Logs are billed at $49/mo per 100K, plus $9 per additional 100K (per a third-party guide).
  - Claims ISO, SOC 2, HIPAA and GDPR compliance.
  - Sources: [TrueFoundry guide](https://www.truefoundry.com/blog/portkey-pricing-guide); [Portkey](https://portkey.ai/features/ai-gateway)
- **Kong AI PII Sanitizer:** Enterprise licence only, self-hosted sidecar. Source: [Kong](https://developer.konghq.com/plugins/ai-sanitizer/)
- **Cloudflare Firewall for AI:** Enterprise paid add-on. Source: [Cloudflare docs](https://developers.cloudflare.com/waf/detections/firewall-for-ai/)
- **Nightfall:** per user/year, in Foundation and Premier editions. Prices are not published. 150 GB of discovery is included. A 7-day proof of value is offered. Sources: [Nightfall pricing](https://www.nightfall.ai/pricing); [Vendr](https://www.vendr.com/marketplace/nightfall)
- **Protecto:** no public tiers, free trial available. Deploys as SaaS (99.9% SLA), VPC, or air-gapped on-prem. Claims SOC 2 Type II, ISO 27001, HIPAA, GDPR, DPDP and CPRA. Sources: [Protecto](https://www.protecto.ai/); [AppSecSanta](https://appsecsanta.com/protecto)
- **Protecto customers:** named customers include **Bank of Muscat**, Inovalon, Automation Anywhere, Citrix, Dell, Ivanti and Nokia. There is also a case study of a bank's private-cloud "sovereign AI gateway". Sources: [Protecto](https://www.protecto.ai/); [Protecto banks](https://www.protecto.ai/industry/sovereign-ai-for-banks/)
- **Lakera:** customers include **Citi** and Dropbox, plus "Fortune 100 tech and finance companies". Citi Ventures invested in the July 2024 Series B. Source: [Fortune](https://fortune.com/2024/07/24/lakera-20-million-funding-ai-chatbot-security)
- **Skyflow:** customers include "credit card platforms", "neo-banks" and insurtechs (e.g. Spoon Money, India), plus a VISA partnership. Source: [Skyflow Series B press (2021)](https://www.skyflow.com/press/data-privacy-api-company-skyflow-raises-45m-series-b-funding-to-help-fintech-and-healthtech-companies-ship-faster)
- **Regional investment bank (reported 2026-09-17):**
  - Deployed eScan Enterprise DLP endpoint agents across analyst workstations, trading floors and mobile devices, to let staff use Claude, Gemini and ChatGPT while blocking client account data, trading strategies, MNPI and deal information.
  - It "stopped hundreds of attempted transmissions".
  - The bank is unnamed.
  - Source: [CFOtech](https://cfotech.news/story/bank-blocks-confidential-data-leaks-to-public-ai-tools)
- **Deployment modes, summarised from the per-vendor findings in Section 1:**

  | Mode | Vendors |
  |---|---|
  | Browser extension | Harmonic, Prompt Security |
  | Endpoint agent | Prompt Security, Purview Endpoint DLP, Nightfall, eScan |
  | SASE/SSE inline proxy | Zscaler, Netskope, Palo Alto AI Access, Cisco AI Access, Cloudflare Gateway, Cato/Aim |
  | Browser isolation | Zscaler |
  | Enterprise browser | Palo Alto Prisma Browser ([video](https://www.paloaltonetworks.com/resources/videos/prisma-browser-secure-genai)) |
  | API / network intercept | Prisma AIRS |
  | Cloud-native API | Bedrock, Model Armor |
  | Gateway plugin | Portkey, Kong, LiteLLM |
  | Vault | Skyflow, Protecto |

### Inferences
- Banks tend to buy this category from their incumbent SASE/DLP vendor (Zscaler, Netskope, Palo Alto, Microsoft) for workforce use, and use a vault or gateway, often self-hosted, for in-house LLM apps. Public bank case studies are rare, probably because of confidentiality.

### Gaps
- Pricing was not found for Lakera/Check Point, Prisma AIRS, Harmonic, Skyflow LLM Vault, Limina, Model Armor (per-token pricing not retrieved) or Purview DSPM for AI (bundled with M365 E5-type licensing; not verified).
- No named Tier-1 bank case study was found for Harmonic, Nightfall, Prompt Security or Skyflow's LLM vault specifically.

## 6. M&A and funding 2024–2026 as demand signal

### Takeaway
Between Sept 2024 and mid-2026, nearly every venture-backed "AI firewall/guardrail" startup was bought by a platform security vendor, typically for $150–650M:
- Robust Intelligence → Cisco
- Protect AI → Palo Alto
- Lakera → Check Point
- Prompt Security → SentinelOne
- Aim → Cato
- CalypsoAI → F5
- Pangea → CrowdStrike
- SPLX → Zscaler
- Acuvity → Proofpoint
- TrojAI → A10

The remaining independents are mostly data-protection-first: Harmonic, Nightfall, Protecto, Skyflow, Limina, Lasso. Presidio moved from Microsoft to community governance in 2026.

### Cited Findings
- **Cisco ← Robust Intelligence:** ~$400M, Sept/Oct 2024. Became Cisco AI Defense. Sources: [BankInfoSecurity](https://www.bankinfosecurity.com/blogs/ai-security-goes-mainstream-as-vendors-spend-heavily-on-ma-p-3953); [DeviDevs](https://devidevs.com/blog/2026-03-19-ai-security-acquisition-wave)
- **Palo Alto Networks ← Protect AI:**
  - Announced April 2025, completed mid-2025. Protect AI CEO Ian Swanson became VP Product, Prisma AIRS.
  - Price: $634.5M per BankInfoSecurity; DeviDevs says "$500–700M".
  - Sources: [PANW press](https://www.paloaltonetworks.com/company/press/2025/palo-alto-networks-completes-acquisition-of-protect-ai); [BankInfoSecurity](https://www.bankinfosecurity.com/blogs/ai-security-goes-mainstream-as-vendors-spend-heavily-on-ma-p-3953)
- **SentinelOne ← Prompt Security:**
  - Announced Aug 2025, closed **2025-09-05**.
  - Consideration per SEC filing: ~$133.6M cash + 1,555,099 Class A shares + 415,109 assumed options, with fair value **$159.3M**. Press reports of a "$250M" deal are contradicted by this fair value.
  - Sources: [SEC 10-Q](https://www.sec.gov/Archives/edgar/data/1583708/000158370825000159/s-20251031.htm); [Dealroom](https://app.dealroom.co/news/note/sentinelone-acquires-prompt-security-for-159-3m-sec-sep-2025); [SiliconANGLE](https://siliconangle.com/2025/08/05/sentinelone-acquires-ai-security-startup-prompt-security/)
- **Cato Networks ← Aim Security:**
  - Announced 2025-09-03, Cato's first acquisition, est. $300–350M.
  - Aim had ~30 staff and had raised $28M. Its product covered employee use of public AI, private AI apps, and the AI development lifecycle.
  - Sources: [Calcalist](https://www.calcalistech.com/ctechnews/article/7pzhe3mrd); [Cato press](https://www.catonetworks.com/news/cato-acquires-aim-security-to-extend-sase-leadership-and-secure-enterprise-ai-transformation/); [SecurityWeek](https://www.securityweek.com/cato-networks-acquires-ai-security-firm-aim-security/)
- **Check Point ← Lakera:**
  - Announced Sept 2025, expected to close Q4 2025, est. ~$300M.
  - Lakera was founded 2021 in Zurich. It becomes Check Point's Global Center of Excellence for AI Security.
  - Lakera had raised a $20M Series B (Atomico lead; Citi Ventures, Dropbox Ventures) in July 2024, for $30M total.
  - Sources: [Check Point press](https://www.checkpoint.com/press-releases/check-point-acquires-lakera-to-deliver-end-to-end-ai-security-for-enterprises/); [Calcalist](https://www.calcalistech.com/ctechnews/article/rj5bc1vige); [Fortune](https://fortune.com/2024/07/24/lakera-20-million-funding-ai-chatbot-security)
- **F5 ← CalypsoAI:**
  - **$180M**, closed **2025-09-26** (Dublin-based). Relaunched as F5 AI Guardrails and AI Red Team.
  - One aggregator dates the deal to January 2026, which conflicts with the SEC filing.
  - Sources: [F5 10-K FY2025](https://www.sec.gov/Archives/edgar/data/1048695/000104869525000157/ffiv-20250930.htm); [GeekWire](https://www.geekwire.com/2025/f5-paying-180m-to-acquire-calypsoai-to-boost-ai-enterprise-security-offerings/); conflicting date in [softwarestrategiesblog](https://softwarestrategiesblog.com/2026/03/28/agentic-ai-security-startups-funding-mna-rsac-2026/)
  - F5 also later acquired SurePath AI (shadow-AI detection). Source: [softwarestrategiesblog](https://softwarestrategiesblog.com/2026/03/28/agentic-ai-security-startups-funding-mna-rsac-2026/)
- **CrowdStrike ← Pangea:** ~$260M, completed ~2025-09-26. Became "AI Detection and Response (AIDR)". Sources: [SecurityWeek](https://www.securityweek.com/crowdstrike-to-acquire-pangea-to-launch-ai-detection-and-response-aidr/); [CrowdStrike IR](https://ir.crowdstrike.com/news-releases/news-release-details/crowdstrike-acquire-pangea-secure-every-layer-enterprise-ai)
- **Zscaler ← SPLX (SplxAI):** closed 2025-10-31 for $40.6M cash (AI red teaming and runtime). Sources: [SecurityWeek](https://www.securityweek.com/zscaler-acquires-ai-security-company-splx/); [Zscaler 10-K FY2026](https://www.sec.gov/Archives/edgar/data/0001713683/000171368326000157/zs-20260731.htm)
- **2026 deals:**
  - Proofpoint ← Acuvity (2026-02-12; agentic workspace AI security and governance). Source: [Proofpoint press](https://www.proofpoint.com/us/newsroom/press-releases/proofpoint-acquires-acuvity-deliver-ai-security-and-governance-across)
  - A10 Networks ← TrojAI (AI red-teaming and runtime protection). Source: [softwarestrategiesblog](https://softwarestrategiesblog.com/2026/03/28/agentic-ai-security-startups-funding-mna-rsac-2026/)
- **Other deals listed by BankInfoSecurity:** Tenable ← Apex Security, Snyk ← Invariant Labs, Coralogix ← Aporia, and Varonis ← SlashNext. Sources: [BankInfoSecurity](https://www.bankinfosecurity.com/blogs/ai-security-goes-mainstream-as-vendors-spend-heavily-on-ma-p-3953); [CyberScoop](https://cyberscoop.com/check-point-lakera-acquistion-ai-security/)
- **Funding of the remaining independents:**
  - **Harmonic Security:** $17.5M Series A (Oct 2024, Next47 lead) after a $7M seed, for $26M+ total. No 2025–26 round found. Source: [Harmonic press](https://www.harmonic.security/resources/harmonic-security-raises-17-5-million-series-a-to-accelerate-zero-touch-data-protection-to-market)
  - **Nightfall:** $20.3M Series A (2019) and $40M Series B (Sept 2022), for ~$60.3M total. A search-engine summary claimed a "$64M Series A on June 29, 2026 (total $85M)", but Nightfall's press page shows no 2026 round, so treat it as **unverified**. Sources: [Nightfall press](https://www.nightfall.ai/press); [Nightfall Series B](https://www.nightfall.ai/blog/nightfall-ai-raises-40-million-series-b-to-expand-cloud-data-protection-platform); [StartupHub](https://www.startuphub.ai/startups/nightfall-ai)
  - **Skyflow:** $45M Series B (Oct 2021, Insight Partners) plus a $30M extension (2024, Khosla). Sources: [Skyflow press](https://www.skyflow.com/press/data-privacy-api-company-skyflow-raises-45m-series-b-funding-to-help-fintech-and-healthtech-companies-ship-faster); [Finovate](https://finovate.com/data-privacy-vault-skyflow-secures-30-million-in-new-funding/)
  - **Lasso Security:** ~$28M total (Samsung Next, Singtel Innov8, others). Independent as of mid-2026. Source: [Tracxn](https://tracxn.com/d/companies/lasso/__HqvLCtSQFd1uNOpZbRkU88jAvcDShMzs3niRzG00h9M)
  - **Private AI → Limina:** rebranded 2026-03-05, still independent. Source: [Limina](https://www.getlimina.ai/en/blog/private-ai-rebrands-limina-sensitive-data-privacy)
- **Adjacent open-source move:** Presidio transferred from Microsoft to the Data Privacy Stack community org in 2026. Source: [Data Privacy Stack blog](https://dataprivacystack.org/blog/presidio-project-joins-data-privacy-stack/)
- **Analyst view:** the 2025 Gartner AI TRiSM Market Guide names "AI Runtime Inspection and Enforcement" as a category. Representative vendors named there include HiddenLayer, Daxa, AIShield (Bosch) and Lasso. Sources: [Gartner doc page](https://www.gartner.com/en/documents/6185655); [Lasso](https://www.lasso.security/blog/gartner-names-lasso-security-as-a-representative-vendor); [HiddenLayer](https://www.hiddenlayer.com/news/hiddenlayer-recognized-in-2025-gartner-market-guide-for-ai-trust-risk-and-security-management-ai-trism)
- **Commentary on the wave:** one commentator argues "every major independent AI security startup disappeared into an enterprise vendor" within ~18 months, leaving the mid-market underserved. Source: [DeviDevs, 2026-03-19](https://devidevs.com/blog/2026-03-19-ai-security-acquisition-wave)

### Inferences
- **What the prices say.** Buyers paid roughly 5–12× the capital raised (e.g. Aim: $28M raised → ~$300–350M; Prompt Security: $159M fair value against a small raise). That signals strong strategic demand for AI-traffic inspection. The buyers are *network/endpoint platforms*, which suggests the market sees this as a feature of SASE/EDR rather than a standalone category.
- **What is still independent.** The standalone survivors are concentrated in data-centric privacy (vaults, de-identification, GenAI DLP). That is the segment closest to a "privacy middleware for banks" position.

### Gaps
- Veeam's reported ~$1.7B acquisition of Securiti AI (which markets LLM firewalls and data-command-center products) came up in search results, but I could not retrieve a primary source for price or date.
- Protecto's funding history was not found.
- Palo Alto's exact Protect AI price was not confirmed from an SEC filing.
