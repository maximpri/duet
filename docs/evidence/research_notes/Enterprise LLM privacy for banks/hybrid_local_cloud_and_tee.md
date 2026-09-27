# Hybrid local+cloud LLM architectures and confidential computing for keeping sensitive data from frontier-model providers (state as of September 2026)

Scope note: research conducted 2026-09-25. Primary sources (arXiv, vendor engineering blogs, official docs) are preferred; aggregator or secondary sources are flagged inline. Dates are given for every fact that could have changed.

## Q1. Local-remote collaboration research: Minions/MinionS, Secure Minions, PAPILLON, local sanitization/pseudonymization, split inference. What are the measured privacy leakage and quality loss?

### Takeaway
Local-remote collaboration works well for cutting cost on long-document question answering: MinionS keeps 97.9% of GPT-4o accuracy at 5.7x lower cloud cost. That result is about cost, not privacy. The Minions authors say explicitly that they do not address privacy. Privacy-focused delegation such as PAPILLON leaks less but does not reach zero: the best configuration keeps 85.5% quality while still leaking 7.5% of private information units. Newer work (2026) shows that stripping PII misses quasi-identifiers, and that web-search-equipped LLM agents can re-identify people from anonymized text. Split inference (sending hidden states to a server) is badly broken: activations can be inverted back to the input even when defenses are applied. The only variant with a hard guarantee is Secure Minions, and its guarantee comes from a GPU trusted execution environment (TEE), not from the protocol.

### Cited Findings

#### Minion / MinionS (Stanford Hazy Research, Feb 2025)
- Setup: a small on-device LM with access to local data talks to a cloud frontier LM to solve financial, medical and scientific reasoning tasks over long documents. The goal is lower cloud inference cost at preserved quality. — [arXiv 2502.15964](https://arxiv.org/abs/2502.15964)
- Minion (one local model chatting with the cloud model): 30.4x lower remote cost, recovering about 87% of frontier-only performance. MinionS (the cloud model decomposes the task into subtasks that the local model runs in parallel over chunks): 97.9% of remote-only accuracy at about 17.5% of the cost (5.7x cheaper). — [Hazy Research blog, 2025-02-24](https://hazyresearch.stanford.edu/blog/2025-02-24-minions); [retrospective, 2026-05-15](https://hazyresearch.stanford.edu/blog/2026-05-15-minions-to-openjarvis-retrospective)
- Models and benchmarks. Local: Llama 3.2 1B/3B, Llama 3.1 8B, Qwen2.5 1.5B/3B/7B. Remote: mainly GPT-4o. Benchmarks: FinanceBench (~142.9K-token contexts), LongHealth (~120.1K), QASPER (~54.3K). — [arXiv 2502.15964 HTML](https://arxiv.org/html/2502.15964v1)
- Local model size matters a lot. 1B local models were close to useless. 3B reached 93.4% of remote-only performance at 16.6% of the cost, and 8B reached 97.9% at about 18%. — [arXiv 2502.15964 HTML](https://arxiv.org/html/2502.15964v1)
- Local-model failure modes: performance fell 56 percentage points when instructions had multiple sequential steps instead of one, and fell 13% when context grew from under 1K to over 65K tokens on basic extraction. — [arXiv 2502.15964 HTML](https://arxiv.org/html/2502.15964v1)
- **Privacy is not a goal of Minions.** The paper states: "we do not address privacy concerns, though these privacy techniques can be used in conjunction with MinionS." The local model reads the full context and sends a compressed/extracted version to the cloud, so derived content from the sensitive data does leave the device. — [arXiv 2502.15964 HTML](https://arxiv.org/html/2502.15964v1)
- Adoption: same-day Ollama integration, AMD Ryzen AI through Lemonade, and Docker built MinionS into Compose/Model Runner. — [Hazy retrospective](https://hazyresearch.stanford.edu/blog/2026-05-15-minions-to-openjarvis-retrospective); [Ollama blog](https://ollama.com/blog/minions); [AMD article](https://www.amd.com/en/developer/resources/technical-articles/2025/minions--on-device-and-cloud-language-model-collaboration-on-ryz.html)
- Hazy's May 2026 retrospective reports these "Intelligence per Watt" results for 2023–2025:
  - Best-of-local win rate against frontier models rose from 23.2% to 71.3%.
  - Local models of 20B parameters or less can answer 88.7% of single-turn chat and reasoning queries.
  - Hybrid local-cloud routing cut energy, compute and cost by 60–80% versus a batched cloud baseline.
  - But "browser-based agentic tasks and coding still favor cloud."
  - The follow-on framework, OpenJarvis v1.0, supports four local engines (Ollama, vLLM, SGLang, llama.cpp) and five cloud engines.
  — [Hazy retrospective, 2026-05-15](https://hazyresearch.stanford.edu/blog/2026-05-15-minions-to-openjarvis-retrospective)

#### Secure Minions (Hazy Research, 2025-05-12)
- Threat model:
  - Untrusted: the cloud provider and datacenter operators, which want access to LLM inputs, outputs or intermediate activations.
  - Trusted: the local client and the silicon vendors NVIDIA and AMD.
  — [Hazy Research "Secure Minions"](https://hazyresearch.stanford.edu/blog/2025-05-12-security)
- Protocol:
  1. Ephemeral key exchange with a CPU enclave.
  2. Attestation that the server runs genuine AMD SEV-SNP with a specific VM ID and a real H100, both in confidential mode, checked against a firmware-sealed launch-measurement hash.
  3. End-to-end encrypted and signed messages with nonce-based replay protection.
  4. Prompts are decrypted only inside the enclave.
  — [Hazy Research "Secure Minions"](https://hazyresearch.stanford.edu/blog/2025-05-12-security)
- Measured overhead:
  - Attestation adds 2–6 s at the start of each chat; encryption adds milliseconds.
  - At batch sizes 4–16: about 28.5% slowdown for 3B models, about 4.6% for 8B, "almost free" above 10B.
  - 32B models: under 1% latency overhead and under 0.3% QPS impact. 3B models: 22% QPS overhead.
  - Overhead grows at batch sizes of 32 and above.
  — [Hazy Research "Secure Minions"](https://hazyresearch.stanford.edu/blog/2025-05-12-security)
- Stated limitations: the prototype "has not undergone a third-party audit." The demo runs on Azure, so it requires trust in Azure unless the user verifies the remote VM code or brings their own virtualization stack and trusted OS. — [Hazy Research "Secure Minions"](https://hazyresearch.stanford.edu/blog/2025-05-12-security)
- Ollama described Secure Minion as turning "an H100 into a secure enclave, where all memory and computation are encrypted — even root users can't access plaintext." — [Ollama on X](https://x.com/ollama/status/1929935933547094460)

#### PAPILLON: privacy-conscious delegation (NAACL 2025; arXiv Oct 2024)
- Defines the "Privacy-Conscious Delegation" task. A local model runs:
  - a Prompt Creator, which rewrites the user query into a privacy-preserving prompt for the remote model;
  - an Information Aggregator, which combines the remote response with the original query.
  The pipeline is optimized with DSPy-style prompt optimization. — [arXiv 2410.17127](https://arxiv.org/abs/2410.17127)
- Benchmark: PUPA, with 901 WildChat instances (PUPA-TNB 237, PUPA-New 664) covering job/visa applications, financial information and quoted messages that contain PII. — [arXiv 2410.17127 HTML](https://arxiv.org/html/2410.17127)
- Results with GPT-4o-mini as the remote model, on PUPA-TNB (quality / leakage):

  | Local model | Quality | Leakage |
  |---|---|---|
  | Llama-3.1-8B | 85.5% | 7.5% |
  | Mistral-Small | 85.3% | 27.4% |
  | Llama-3-8B | 79.3% | 21.2% |
  | Mistral-7B | 77.6% | 11.9% |
  | Llama-3.2-3B | 60.9% | 24.9% |
  | Llama-3.2-1B | 58.0% | 39.3% |

  Baselines: unredacted GPT-4o-mini scored 88.2% quality; a redacted-prompt GPT-4o-mini baseline scored 77.2% quality at 0% leakage. — [arXiv 2410.17127 HTML](https://arxiv.org/html/2410.17127)
- How the metrics are defined: quality is an LLM judge asking whether the output is "at least as good as" the target. Leakage is the percentage of private information units present in the prompt sent to the remote model, judged by an LLM with 86% agreement with humans. — [arXiv 2410.17127 HTML](https://arxiv.org/html/2410.17127)
- Stated limitations: explicit PII only (implicit disclosures not handled); single-turn only; the rewriter sometimes adds extraneous or intent-distorting content; there is a "large margin" to proprietary-model quality. — [arXiv 2410.17127 HTML](https://arxiv.org/html/2410.17127)

#### 2025–2026 follow-ups on local sanitization, pseudonymization and delegation
- **PUPA-SD / k-anonymity (Aug 2026; the PAPILLON authors).** Privacy risk "does not stem solely from explicit identifiers but also PII-free self-disclosures," which leave users identifiable through combinations of quasi-identifiers. The authors add LLM-estimated k-anonymity to delegation. Optimizing PAPILLON on PUPA-SD gave the best privacy-utility balance for Llama-3.2-3B, but smaller models struggled with joint optimization. Accepted at the HAIPS workshop, COLM 2026. — [arXiv 2608.09140](https://arxiv.org/abs/2608.09140)
- **"Need to Know" (June 2026).** Trains a query rewriter with reinforcement learning, grounded in Contextual Integrity, to keep task-necessary spans and drop unnecessary sensitive ones. Tested on DelegateCI-Bench (3,167 samples, 11 tasks, including WildChat and medical data). Reports up to +10.1 average utility over on-device baselines. Argues that type-based PII redaction both over-discloses untyped sensitive content and over-removes answer-bearing information. — [arXiv 2606.04067](https://arxiv.org/abs/2606.04067)
- **DR-SL "dehydrate-rehydrate" de-identification for cloud-local inference (Sept 2026).**
  - Worst-case leakage fell from 0.457 to 0.304.
  - On a mixed benchmark, 67.5% of instances were released automatically at zero measured leakage.
  - The authors concede the theoretical bounds are "near-vacuous" and that release safety "rests on empirical calibration, the hard line, and human review."
  — [arXiv 2609.14883](https://arxiv.org/abs/2609.14883)
- **General pseudonymization framework for cloud LLMs (Feb 2025).** Replaces private information in controlled text generation and restores it afterwards. The abstract claims an "optimal balance" between privacy and utility but gives no headline numbers. — [arXiv 2502.15233](https://arxiv.org/abs/2502.15233)
- **"Privacy gatekeepers" (Aug 2025).** A lightweight local model filters sensitive information before the cloud call. Human-subject experiments report minimal overhead and no loss of response quality. — [arXiv 2508.16765](https://arxiv.org/abs/2508.16765)
- **Agentic re-identification (May 2026).** LLM agents with web search can turn "weak contextual cues" into cross-referenceable evidence and re-identify anonymized people (tested on real interview transcripts). The paper proposes AURA, a mask-reconstruct defense, but the abstract gives no attack success rates. — [arXiv 2605.30848](https://arxiv.org/abs/2605.30848)
- Other work in this line: Prεεmpt, prompt sanitization — [arXiv 2504.05147](https://arxiv.org/pdf/2504.05147); a survey of privacy-preserving prompt engineering — [arXiv 2404.06001](https://arxiv.org/pdf/2404.06001); and a proposed semantic metric for sanitization privacy — [arXiv 2605.02977](https://arxiv.org/pdf/2605.02977).

#### Split inference (local runs the first layers, the server runs the rest)
- **ActInv (ACM CCS '26, May 2026).** Reconstructs the client's input from the intermediate activations sent to the server, with "high-fidelity reconstructions, even in the presence of common perturbation-based defenses such as Gaussian noise injection and activation sparsification." How vulnerable an activation is varies by layer. The authors conclude that split inference's privacy assumptions "warrant serious reconsideration." — [arXiv 2605.23158](https://arxiv.org/abs/2605.23158)
- Related 2025 results:
  - "Depth Gives a False Sense of Privacy: LLM Internal States Inversion" — [arXiv 2507.16372](https://arxiv.org/pdf/2507.16372)
  - Work showing LLMs are almost-surely injective, with the SipIt prompt-recovery attack (cited in the ActInv search summary) — [arXiv 2605.23158 HTML](https://arxiv.org/html/2605.23158v1)

### Inferences
- Minions-style protocols do reduce how much raw data goes to the cloud, since the local model reads the document and sends extracted snippets. But nothing constrains what those snippets contain. For a bank, MinionS is a cost-and-exposure-reduction tool, not a confidentiality control.
- Local-rewriting approaches have a floor: in PAPILLON's best configuration about 1 in 13 private units still leaks, and that is measured only for explicit PII. The 2026 papers (quasi-identifiers, agentic re-identification) suggest real leakage against a motivated adversary is higher than the headline numbers.
- Across Minions and PAPILLON, local model capability is the binding constraint. 1B–3B models perform far worse than 8B-class models on both quality and leakage, so an on-device "privacy filter" needs at least an 8B-class model to be credible.
- Among collaboration protocols, only a hardware TEE (Secure Minions) gives a cryptographic confidentiality guarantee against the cloud operator. Everything else is best-effort statistical filtering.

### Gaps
- No paper found that measures local-sanitization leakage or quality on **code or agentic software-engineering** tasks, for example leakage of proprietary source code, secrets or internal identifiers when a local model summarizes code for a cloud model. All benchmarks found are QA/chat (PUPA, DelegateCI-Bench, FinanceBench, etc.).
- Secure Minions publishes no cost figures and no end-to-end MinionS-plus-TEE accuracy comparison.
- AURA and the pseudonymization framework give no quantitative leakage or success rates in their abstracts; full-text numbers were not retrieved.

## Q2. Apple Private Cloud Compute and the on-device vs. server split; Google Private AI Compute (2025); Samsung / Microsoft / Meta equivalents. What guarantees do they give?

### Takeaway
Apple's PCC (June 2024) set the template that others followed:
- stateless processing;
- no privileged runtime access;
- non-targetability;
- devices that send data only to attested nodes whose software images appear in a public append-only transparency log.

Google's Private AI Compute (Nov 2025) and Meta's Private Processing (Apr 2025; extended to AI glasses Sept 2026) copy this pattern: attestation, OHTTP relays and transparency ledgers. In 2026 the two most significant developments are:
- Apple runs a Google-derived Gemini model for Siri on device plus PCC (confirmed Jan 2026).
- Google Cloud announced it hosts Apple PCC using Intel TDX and NVIDIA Blackwell confidential computing (June 2026).

In both cases a frontier model runs on third-party infrastructure while the operator is cryptographically excluded. Independent audits show these systems are hard to build correctly: Trail of Bits found 8 high-severity issues in WhatsApp's system before launch.

### Cited Findings

#### Apple Private Cloud Compute
- Announced 2024-06-10. Five core requirements:
  1. Stateless computation: "personal data leaves no trace in the PCC system."
  2. Enforceable guarantees, with no reliance on external components such as TLS terminators.
  3. No privileged runtime access: no remote shells or interactive debugging.
  4. Non-targetability: an attacker cannot steer a specific user's requests to compromised nodes without broad system compromise.
  5. Verifiable transparency.
  — [Apple Security Research blog](https://security.apple.com/blog/private-cloud-compute/)
- Mechanism:
  - Apple Intelligence does simpler tasks on device and offloads larger-model requests to PCC.
  - The device encrypts each request directly to the public keys of validated PCC nodes, so intermediate services cannot read it.
  - Nodes are custom Apple-silicon servers with Secure Enclave and Secure Boot.
  - Production images are published within 90 days to an append-only, cryptographically tamper-proof transparency log.
  - Apple provides a Virtual Research Environment, and sepOS/iBoot are released in plaintext.
  — [Apple Security Research blog](https://security.apple.com/blog/private-cloud-compute/)
- 2026-01-29: Tim Cook said on the earnings call that the Gemini-collaboration Siri models will "continue to run on the device and run in Private Cloud Compute, and maintain our industry-leading privacy standards." — [9to5Mac](https://9to5mac.com/2026/01/29/apple-confirms-gemini-powered-siri-will-use-private-cloud-compute/); see also the [Google–Apple joint statement](https://blog.google/company-news/inside-google/company-announcements/joint-statement-google-apple/) and [CNBC, 2026-01-12](https://www.cnbc.com/2026/01/12/apple-google-ai-siri-gemini.html)
- Reported but not confirmed by Apple: the custom model has about 1.2 trillion parameters, the deal is worth about $1B/year, and it runs on PCC rather than Google Cloud so Google does not see requests. — [Introl blog (secondary)](https://introl.com/blog/apple-google-gemini-partnership-siri-ai-infrastructure-2026); [MacUser (secondary)](https://macuser.org.uk/2026/09/14/apple-siri-google-gemini-deal-privacy-explained/)
- 2026-06-11: Google Cloud announced it hosts Apple PCC. The stack is Intel TDX, NVIDIA Blackwell GPUs in confidential-computing mode, the Google Titan/Titanium root of trust, and an **open-source host stack** for independent verification. Google says it cannot see workload data inside the TEEs. — [Google Cloud blog "Powering the next era of Confidential AI"](https://cloud.google.com/blog/products/identity-security/powering-the-next-era-of-confidential-ai/); [Google Cloud blog, 2026-06-23](https://cloud.google.com/blog/products/identity-security/verifiable-trust-in-the-ai-era-whats-new-in-confidential-computing)

#### Google Private AI Compute
- Announced 2025-11-11.
  - Runs on Google's own stack: custom TPUs plus "Titanium Intelligence Enclaves (TIE)."
  - Remote attestation and encryption connect the device to a "hardware-secured sealed cloud environment."
  - Google claims data is "accessible only to you and no one else, not even Google."
  - First uses: Magic Cue on Pixel 10 and Recorder summarization.
  — [Google blog](https://blog.google/innovation-and-ai/products/google-private-ai-compute/)
- Secondary coverage: Google will publish a tamper-proof public record of server software so devices can verify it before sending data. — [Datamation](https://www.datamation.com/artificial-intelligence/google-private-ai-compute/)
- The Google blog post itself does not explicitly promise non-retention and does not cite an independent audit (per the fetched page). — [Google blog](https://blog.google/innovation-and-ai/products/google-private-ai-compute/)

#### Meta Private Processing (WhatsApp 2025, AI glasses 2026)
- WhatsApp version (2025-04-29):
  - Confidential VMs using AMD SEV-SNP on the CPU and H100 confidential computing on the GPU.
  - An OHTTP connection through a third-party relay hides the requester's IP from Meta.
  - An RA-TLS session to the TEE, with measurements cross-checked against a third-party ledger.
  - Non-targetability, and no message storage after the session.
  — [Engineering at Meta](https://engineering.fb.com/2025/04/29/security/whatsapp-private-processing-ai-tools/)
- 2026-09-23 extension to Meta AI glasses:
  - Relays run by Fastly or Cloudflare.
  - CVM images registered on an "append-only, publicly-witnessed transparency ledger."
  - "Blind-signed tokens fetched on randomized schedules" so authentication cannot be linked to requests.
  - Supports stateful features (long-term recall).
  - The design "assumes an adversarial environment inside our own data centers."
  — [Engineering at Meta](https://engineering.fb.com/2026/09/23/security/private-processing-meta-ai-glasses/)
- Trail of Bits pre-launch audit of WhatsApp private inference: **28 issues, 8 high-severity**. Examples:
  - Environment variables loaded after measurement, allowing LD_PRELOAD injection.
  - ACPI tables excluded from attestation, letting a hypervisor inject fake devices.
  - Firmware patch levels checked against untrustworthy claims.
  - Attestation reports without nonce or timestamp, allowing indefinite replay.
  - Inadequate protection against physical attacks under the SEV-SNP threat model.
  - Gaps in CVM image reproducibility.
  Meta fixed 16 issues, partially fixed 4, and left 8 low/informational. — [Security Boulevard (Trail of Bits write-up), Apr 2026](https://securityboulevard.com/2026/04/what-we-learned-about-tee-security-from-auditing-whatsapps-private-inference/)

#### Samsung and Microsoft
- Samsung Galaxy AI mixes on-device features with cloud features, including Gemini. A user setting, "Process Data Only on Device," restricts some AI tasks to local processing. — [PromptQuorum (secondary)](https://www.promptquorum.com/local-llms/galaxy-s26-local-ai-on-device-2026); [Wikipedia: Galaxy AI](https://en.wikipedia.org/wiki/Galaxy_AI)
- Microsoft's only listed confidential *LLM-class* inference service is still "Confidential inferencing with the Azure OpenAI Whisper model." It supports TEEs, encrypted prompts, user anonymity and OHTTP. The Azure product overview page was dated 2026-06-10. — [Microsoft Learn](https://learn.microsoft.com/en-us/azure/confidential-computing/overview-azure-products)

### Inferences
- Several frontier-grade assistants now ship this consumer pattern at scale: device, then attested stateless enclave, then a transparency log. Apple PCC even runs a Gemini-derived model with Google excluded, which shows a frontier model *can* be served without the model vendor or the cloud operator seeing prompts. That is the reference architecture a bank would want from a coding-model provider.
- The Trail of Bits findings show that attestation completeness (measuring everything that is loaded, binding nonces, validating patch levels) is where real deployments fail. "Uses TEEs" is not a guarantee without an audit.

### Gaps
- No primary Samsung document was found describing a PCC-equivalent server-side attested enclave for Galaxy AI. Only an on-device toggle is documented, and only in secondary sources.
- No Microsoft PCC-equivalent was found for Copilot or Azure OpenAI GPT models; confidential inferencing appears limited to Whisper as of mid-2026.
- No NCC Group or other third-party audit of Google Private AI Compute was confirmed; the search budget ran out before this could be checked.
- Whether Apple's Gemini-based Siri model runs on Apple-silicon PCC nodes, on the Google-hosted TDX/Blackwell PCC, or both was not established from primary sources.

## Q3. Confidential computing and TEEs for LLM inference: how each works, measured overhead, commercial products, threats addressed, and what remains

### Takeaway
GPU confidential computing is available on NVIDIA H100, H200, B200 and GB200. It pairs a CPU confidential VM (Intel TDX, AMD SEV-SNP or Arm CCA) with a GPU in confidential mode. On H100, CPU–GPU traffic goes through encrypted bounce buffers; Blackwell adds TEE-I/O and encrypted NVLink.

Overhead is now small when configured well (1–4% on B200 per NVIDIA and an Aug 2026 paper) but often 13–40% with stock stacks, especially for small models and high concurrency.

It protects prompts and model weights against the cloud operator and the serving company's own staff, if the serving code is attested and transparent. What remains:
- trust in NVIDIA, Intel and AMD;
- trust that the attested code is honest;
- physical memory-bus attacks: TEE.fail (Oct 2025) and DDRop (Sept 2026) break TDX and SEV-SNP for under $1,000 of hardware. GPU HBM is not reachable by an interposer, but TEE.fail used extracted host keys to undermine GPU attestation.

Fully homomorphic encryption (FHE) remains about 18 s per generated token for an 8B model, so it is not practical for interactive use.

### Cited Findings

#### How GPU confidential computing works
- The H100 was the first GPU with confidential computing support. It requires a CPU TEE: Intel TDX, AMD SEV-SNP or Arm CCA. — [NVIDIA Technical Blog](https://developer.nvidia.com/blog/confidential-computing-on-h100-gpus-for-secure-and-trustworthy-ai/)
- On H100, inputs and outputs crossing PCIe are encrypted through bounce buffers. The main bottleneck is CPU–GPU I/O, not GPU compute. — [Phala benchmark post, 2024-09-05](https://phala.com/posts/confidential-computing-on-nvidia-h100-gpu-a-performance-benchmark-study); [arXiv 2409.03992](https://arxiv.org/pdf/2409.03992)
- NVIDIA describes Blackwell as "the first TEE-I/O capable GPU in the industry," with inline protection over NVLink. — [NVIDIA Blackwell architecture page](https://www.nvidia.com/en-us/data-center/technologies/blackwell-architecture/)
- B200/GB200 add NVLink encryption to PCIe encryption. — [Spheron (secondary)](https://www.spheron.network/blog/confidential-gpu-computing-nvidia-tee-encrypted-vram/)
- NVIDIA (2026-09-22): the production stack uses memory-encrypted confidential VMs, confidential GPUs and encrypted NVLink. Host-to-device transfers still use a "software encrypted bounce buffer," and NVLS multicast is unavailable in B200 confidential configurations. — [NVIDIA Technical Blog](https://developer.nvidia.com/blog/enabling-private-high-performance-production-ai-inference-with-nvidia-confidential-computing/)

#### Measured performance overhead (chronological)
- **Sept 2024, H100** (Llama-3.1-8B, Phi-3-14B, Llama-3.1-70B 4-bit): average overhead under 7%. Time to first token (TTFT) was about +6% for 8B and negligible for 70B; inter-token latency was about +3% for 8B and near zero for larger models. — [Phala / arXiv 2409.03992](https://phala.com/posts/confidential-computing-on-nvidia-h100-gpu-a-performance-benchmark-study)
- **Sept 2025, CPU vs. GPU TEEs** (Llama2 7B/13B/70B): H100 confidential mode cost 4–8% throughput, shrinking with batch and input size. CPU TEEs (TDX, SGX) cost under 10% throughput and under 20% latency; Intel AMX helps. The authors argue CPU TEEs can be more cost-effective. — [arXiv 2509.18886 (Chrapek, Copik, Mettaz, Hoefler)](https://arxiv.org/abs/2509.18886)
- **May 2025, Secure Minions on H100 + SEV-SNP:** about 28.5% slowdown at 3B, about 4.6% at 8B, under 1% at 32B; 2–6 s of attestation per chat. — [Hazy Research](https://hazyresearch.stanford.edu/blog/2025-05-12-security)
- **May 2026, H100 + Intel TDX** (Mistral-7B, Qwen3-30B-A3B):
  - TTFT up 21.8% and 27.8%; token throughput down 17.7% and 21.1% at fixed request rate.
  - 11.5–20.2% throughput gap under closed-loop concurrency; larger models saturate earlier.
  - Suggested planning heuristic: reserve 15–25% extra capacity.
  — [arXiv 2607.19353](https://arxiv.org/abs/2607.19353)
- **June 2026, Blackwell + TDX, "The Serialized Bridge":**
  - GPU compute is at near-parity (0.998x), but serving loses 13–27% of throughput because the VM–GPU secure copy path lacks CUDA-stream concurrency, blocks async transfers, and has high fixed costs for small copies.
  - A scheduling flag recovers 57% of the gap; a worker-thread drain recovers up to 92%.
  - Also measured: a +131% KV-restore penalty and a 34x model-load slowdown.
  - Confidential multi-GPU NVSwitch reached 510 GB/s NVLink P2P.
  — [arXiv 2606.23969](https://arxiv.org/abs/2606.23969)
- **Aug 2026, B200 + TDX:** about 1–3% throughput overhead "when the stack is configured correctly," but stock stacks incur 30–40% penalties from avoidable configurations. GPU compute, energy and usable memory are unaffected. Overhead is a fixed per-host-operation cost (amortized by batch) plus a cost proportional to encrypted NVLink traffic. — [arXiv 2608.26575](https://arxiv.org/abs/2608.26575)
- **Sept 2026, NVIDIA on 8x B200 with DeepSeek-R1:** 96.1–98.2% of baseline output-token throughput retained, with 1.2–4.3% added per-token latency. — [NVIDIA Technical Blog](https://developer.nvidia.com/blog/enabling-private-high-performance-production-ai-inference-with-nvidia-confidential-computing/)

#### Cloud offerings
- **Azure.**
  - Confidential GPU VMs (NCCadsH100v5) pair AMD SEV-SNP with H100. — [Microsoft Learn (dated 2026-06-10)](https://learn.microsoft.com/en-us/azure/confidential-computing/overview-azure-products); [GA announcement](https://techcommunity.microsoft.com/blog/azureconfidentialcomputingblog/general-availability-azure-confidential-vms-with-nvidia-h100-tensor-core-gpus/4242644)
  - Confidential inferencing (Azure OpenAI **Whisper** only) uses Oblivious HTTP with HPKE. Prompts pass through Azure Front Door and the Azure OpenAI load balancer to OHTTP gateways inside confidential GPU VMs, and are decrypted only in the TEE. — [Azure AI Confidential Inferencing Preview](https://techcommunity.microsoft.com/blog/azure-ai-foundry-blog/azure-ai-confidential-inferencing-preview/4248181)
  - No GPT-family confidential inferencing is listed as of June 2026. — [Microsoft Learn](https://learn.microsoft.com/en-us/azure/confidential-computing/overview-azure-products)
- **Google Cloud.**
  - A3 Confidential VMs combine AMD SEV-SNP with H100. — [Ubuntu/Canonical](https://ubuntu.com/blog/ubuntu-confidential-vms-now-available-on-google-cloud-a3-with-nvidia-h100-gpus)
  - Announced 2026-06-23:
    - Confidential G4 VMs (RTX PRO 6000 Blackwell + AMD SEV), in preview.
    - Confidential Space with H100 support, GA.
    - Intel Trust Authority integration for attestation independent of the cloud provider, GA.
    - Open-source Prompt Encryption SDKs for end-to-end encrypted prompts over attested TLS, GA.
    — [Google Cloud blog](https://cloud.google.com/blog/products/identity-security/verifiable-trust-in-the-ai-era-whats-new-in-confidential-computing)
- **AWS.** Nitro Enclaves are CPU-and-memory-only, with no GPU passthrough. AWS's own LLM sample runs Bloom-560m inside an enclave. — [aws-samples/aws-nitro-enclaves-llm](https://github.com/aws-samples/aws-nitro-enclaves-llm); [VoltageGPU comparison (competitor, secondary)](https://voltagegpu.com/compare/aws-nitro-enclaves-vs-confidential-gpu)
- **Frontier model delivered on-premises inside a TEE (Google).**
  - Gemini on Google Distributed Cloud (air-gapped) uses NVIDIA Blackwell confidential computing to protect prompts and fine-tuning data. GDC air-gapped holds US Secret/Top Secret authorization. — [AI Magazine](https://aimagazine.com/articles/nvidia-google-cloud-using-gemini-ai-for-regulated-sectors); [SiliconANGLE, 2025-08-27](https://siliconangle.com/2025/08/27/gemini-ai-lands-google-distributed-cloud-secure-premises-adoption/); [Google Cloud blog](https://cloud.google.com/blog/products/ai-machine-learning/run-gemini-and-ai-on-prem-with-google-distributed-cloud)
  - 2026-04-22: a Dell-built, Google-certified appliance with 8 NVIDIA GPUs (via Cirrascale) runs "full blown Gemini" disconnected from Google. The model lives only in volatile memory ("As soon as the power is off, the model is gone"), tampering triggers shutdown, and confidential computing keeps the weights from the customer. Preview at announcement, GA expected June/July 2026. Target customers include financial services. — [VentureBeat](https://venturebeat.com/technology/googles-gemini-can-now-run-on-a-single-air-gapped-server-and-vanish-when-you-pull-the-plug)

#### Model-provider research: Anthropic
- "Confidential Inference via Trusted Virtual Machines" (2025-06-18), design sketch:
  - A small, reviewed and signed "trusted loader" VM decrypts data and talks to accelerators.
  - The frequently changing inference server stays untrusted.
  - Weights are encrypted at rest, and a keyserver releases keys only after TPM-based attestation.
  - Intended to protect against malicious hypervisors, Anthropic insiders and the cloud provider.
  - Stated status: "just a sketch of our research… too soon to forecast how it will evolve into specific designs or features." Accelerators lack full confidential-computing support.
  — [Anthropic](https://www.anthropic.com/research/confidential-inference-trusted-vms)
- A related whitepaper was co-published with Irregular. — [Irregular](https://www.irregular.com/publications/confidential-inference-systems)

#### Commercial vendors
- **Edgeless Systems (Privatemode, Continuum).**
  - Privatemode AI launched 2025-02-19. — [Edgeless blog](https://www.edgeless.systems/blog/what-is-privatemode)
  - 2026-03-12: NVIDIA Blackwell support with remote attestation and quantum-resistant cryptography. It serves open-weight models such as Kimi K2.5. Customers include Capgemini (confidential coding assistants). The underlying technology already secures Germany's ePA health record for 50M patients. — [Edgeless press release](https://www.edgeless.systems/press-release-edgeless-systems-nvidia-confidential-ai)
  - The site lists these model families: Kimi, GLM, OpenAI GPT (presumably gpt-oss, not confirmed), Qwen, Mistral, DeepSeek. It lists coding integrations with Claude Code, VS Code, Zed, Xcode and OpenCode. Other claims: EU-hosted, ISO 27001, BSI C5:2026, StGB §203, Big Four audits. — [privatemode.ai](https://www.privatemode.ai/)
  - The earlier Continuum platform served Mistral 7B on H100. — [Edgeless Continuum](https://www.edgeless.systems/blog/launching-confidential-llm-platform-continuum-ai)
- **Tinfoil.**
  - Uses NVIDIA Hopper and Blackwell confidential computing (including multi-GPU) with AMD SEV / Intel TDX.
  - Every request is encrypted to an attested enclave and verified client-side. The security-critical infrastructure is open source, with transparency logs.
  - Trusts NVIDIA, AMD and Intel, plus the cloud host. Side-channel and physical threats are not discussed.
  — [Tinfoil technology](https://tinfoil.sh/technology); [Tinfoil inference](https://tinfoil.sh/inference)
  - Tinfoil Containers (2026-03-10) let customers deploy their own models or backends in enclaves. — [Tinfoil blog](https://tinfoil.sh/blog/2026-03-10-tinfoil-containers)
  - A Tinfoil provider exists for the "pi" coding agent. — [GitHub tinfoilsh/pi-provider](https://github.com/tinfoilsh/pi-provider)
- **Fortanix Confidential AI** (2026-03-18). Protects proprietary model weights and data in on-prem "AI factories." Keys are released only after the runtime is attested, which gives "cryptographic guarantees, not contractual ones" against infrastructure operators *and model vendors*. ElevenLabs is the named model provider. — [Fortanix PR](https://www.fortanix.com/company/pr/2026/03/fortanix-confidential-ai-protects-proprietary-model-ip-and-data-for-secure-ai-inference-in-enterprise-ai-factories)
  - A turnkey on-prem platform with HPE and NVIDIA was announced in Oct 2025. — [Fortanix PR, 2025-10](https://www.fortanix.com/company/pr/2025/10/fortanix-brings-secure-and-trusted-agentic-ai-to-cutting-edge-nvidia-confidential-computing-gpus)
- **Opaque Systems** (Opaque Studio, 2025-10-29). Confidential AI agents built on LangGraph plus the Opaque Confidential Runtime, with hardware-signed audit logs and cryptographic policy enforcement. Its vendor-claimed metrics (4–5x faster deployment, 67% lower cost) are unverified. — [SiliconANGLE](https://siliconangle.com/2025/10/29/opaque-introduces-confidential-ai-platform-end-end-privacy-compliance-guarantees/)
- **Red Hat** (2025-10-23). Architecture for private inference with *proprietary* LLMs:
  - Weights ship encrypted in OCI images, and the LLM provider's key broker releases keys only after attestation.
  - The user separately verifies the TEE's software.
  - Delivered via OpenShift sandboxed containers (Kata/CoCo).
  - Status: upstream development, "not yet part of supported Red Hat products."
  — [Red Hat Emerging Technologies](https://next.redhat.com/2025/10/23/enhancing-ai-inference-security-with-confidential-computing-a-path-to-private-data-inference-with-proprietary-llms/)
- **Homomorphic encryption (Zama and others).** CKKS-based Llama-3-8B (Jan 2026, revised June 2026) on 8x RTX PRO 6000:
  - 128 encrypted tokens: 20 s to summarize and **18 s per generated token**.
  - Hybrid mode (4,096 tokens with only the last 128 encrypted): 64 s and 22 s per token.
  — [arXiv 2601.18511](https://arxiv.org/abs/2601.18511)
  - A 2026 practitioner blog puts FHE at about 1,000x slower than plaintext. — [Wavect (secondary)](https://wavect.io/blog/fully-homomorphic-encryption-practical-2026/)

#### Threats addressed and what remains
- **Addressed.** A malicious hypervisor, cloud operator or service-company insider cannot read prompts, outputs or weights in memory, provided attestation covers all loaded code and the client verifies it. — [Anthropic](https://www.anthropic.com/research/confidential-inference-trusted-vms); [Hazy Secure Minions](https://hazyresearch.stanford.edu/blog/2025-05-12-security)
- **Remaining: trust in silicon vendors.** Secure Minions explicitly trusts NVIDIA and AMD ("Jensen and Lisa"). Tinfoil trusts NVIDIA, AMD, Intel and the cloud host. — [Hazy](https://hazyresearch.stanford.edu/blog/2025-05-12-security); [Tinfoil](https://tinfoil.sh/technology)
- **Remaining: physical memory-bus attacks.**
  - **TEE.fail** (Oct 2025): a passive DDR5 interposer costing under $1,000. It exploits deterministic memory encryption in TDX and SEV-SNP to extract keys, including attestation keys from fully updated machines. Forged TDX quotes passed Intel DCAP verification at "UpToDate" status. The researchers also compromised NVIDIA GPU confidential computing by extracting attestation keys from Intel hosts. — [tee.fail](https://tee.fail/); [BleepingComputer](https://www.bleepingcomputer.com/news/security/teefail-attack-breaks-confidential-computing-on-intel-amd-nvidia-cpus/); [The Hacker News](https://thehackernews.com/2025/10/new-teefail-side-channel-attack.html)
  - **DDRop** (2026-09-14; KU Leuven, ETH Zurich, Durham, Google): an *active* DDR5 interposer with about $159 in parts that installs in minutes. It silently drops memory writes. Demonstrated full control of up-to-date Intel TDX, page-relocation attacks on AMD SEV-SNP, and effects on Scalable SGX. NVIDIA GPUs are *not* vulnerable because HBM is inside the package. Intel and AMD called physical attacks out of scope and assigned no CVEs. **Battering RAM** is an earlier active attack limited to DDR4. — [The Hacker News](https://thehackernews.com/2026/09/new-ddrop-attack-breaks-intel-tdx-and.html)
- **Remaining: implementation and attestation gaps.** Trail of Bits found 8 high-severity attestation and measurement flaws in Meta's production TEE inference before launch (details in Q2). — [Security Boulevard](https://securityboulevard.com/2026/04/what-we-learned-about-tee-security-from-auditing-whatsapps-private-inference/)
- **Remaining: the serving code itself.** Confidentiality depends on the attested inference code not logging or exfiltrating. That is why Apple, Meta and Google publish images to transparency logs, and why Anthropic's design isolates a small reviewed "trusted loader" from untrusted serving code. — [Apple](https://security.apple.com/blog/private-cloud-compute/); [Anthropic](https://www.anthropic.com/research/confidential-inference-trusted-vms)

### Inferences
- **Performance is no longer the blocker.** Well-tuned Blackwell deployments cost low single digits. The practical risk is running stock stacks (30–40% penalty) and small models (about 20–30% penalty). Latency-sensitive agent loops with many small copies or KV restores sit in the worst case per the "Serialized Bridge" findings, so coding agents with long multi-turn contexts should be benchmarked specifically.
- **TEEs cover a different threat than local-only processing.** A TEE removes the cloud operator and, with attestation plus transparency, the model-serving company from the trust set. It does not remove NVIDIA, Intel or AMD, or a physically present attacker at the host-memory layer. After TEE.fail and DDRop, CPU-side confidential VMs should be treated as strong against remote and software adversaries but not against insiders with physical access at the datacenter. The vendors themselves say this is out of scope.
- **For the frontier-model problem specifically**, the 2026 market has two options:
  - Open-weight models served in TEEs (Privatemode, Tinfoil). This gives strong confidentiality, but the model is one tier below the frontier.
  - Proprietary frontier models inside TEEs where the provider controls key release: Gemini on GDC appliances, Fortanix, the Red Hat pattern, Apple PCC with Gemini.
  No confidential offering was found for Anthropic or OpenAI frontier models: Anthropic's is a research sketch, and Azure's confidential inferencing is Whisper-only.

### Gaps
- Not found: public overhead measurements for confidential inference on **agentic coding workloads** (long contexts, many tool-call turns, KV-cache offload).
- No details retrieved on **Anjuna**, **Duality**, or Phala's current commercial GPU-TEE API; the search budget was exhausted.
- Whether any OpenAI or Anthropic frontier model is offered with customer-verifiable confidential inference as of Sept 2026 was not found. The absence is inferred from Azure docs and Anthropic's June 2025 "sketch" status, not confirmed.
- A search snippet claimed a FHE Llama-3 at "up to 80 tokens/s" (arXiv 2604.12168). It was not verified and likely refers to a partial or hybrid scheme.
- Whether Blackwell TEE-I/O fully removes the bounce-buffer cost on TDX Connect-capable hosts: NVIDIA's Sept 2026 blog still describes software-encrypted bounce buffers.

## Q4. Enterprise products that route sensitive requests to local/on-prem models and others to cloud; hybrid-AI and private-deployment offerings; open-weight use in banks

### Takeaway
Sensitivity-based routing is mostly sold as an **AI-gateway feature**: detect PII or DLP matches, then redact, block or reroute to a self-hosted model. It is not a verified-privacy architecture. Banks are mainly taking the **private-deployment** route instead:
- HSBC self-hosts Mistral models;
- RBC co-developed Cohere "North for Banking," running on-premises;
- Google offers Gemini on air-gapped GDC appliances (Dell-built, confidential computing);
- IBM watsonx and Red Hat OpenShift serve on-prem Granite and open models.

I found no vendor that publishes measured leakage or quality numbers for its sensitivity router.

### Cited Findings
- **AI gateways.** The gateway inspects the prompt for PII after authenticating the caller and before routing. Actions are redaction (placeholders) or blocking. Some vendors describe switching to "locally contained models (like Llama 3 or 4) when data sensitivity demands it." — [API7.ai](https://api7.ai/blog/ai-gateway-pii-redaction); [Maxim AI](https://www.getmaxim.ai/articles/gateway-level-pii-redaction-before-provider-transmission/); [Gravitee](https://www.gravitee.io/blog/how-to-prevent-pii-leaks-in-ai-systems-automated-data-redaction-for-llm-prompt); [SS&C Blue Prism](https://www.blueprism.com/resources/blog/ai-gateway-pii-sanitization/)
- **HSBC and Mistral** (multi-year partnership, 2025). Access to Mistral's commercial models, including future ones, "through self-hosted AI models that operate on HSBC's internal technology systems." Uses include internal productivity tools, document analysis, translation and "agile development," with plans for credit and lending, onboarding, fraud and AML. — [HSBC media release](https://www.hsbc.com/news-and-views/news/media-releases/2025/hsbc-and-mistral-ai-join-forces-to-accelerate-ai-adoption-across-global-bank); [Finextra](https://www.finextra.com/newsarticle/46986/hsbc-to-adopt-mistral-ai-foundational-models-across-the-bank)
- **Cohere North.**
  - The Cohere stack can be deployed privately on customer compute, including air-gapped environments. — [Cohere private deployments](https://cohere.com/private-deployments); [Cohere docs](https://docs.cohere.com/docs/deployment-options-overview)
  - "North for Banking" was co-developed with RBC so agents can work across bank systems with data kept on-premises. North reportedly needs "as few as two GPUs." — [IntuitionLabs (secondary)](https://intuitionlabs.ai/articles/cohere-enterprise-ai-llm-profile); [Constellation Research](https://www.constellationr.com/insights/news/cohere-launches-north-aims-be-ai-agent-workspace)
- **Google Gemini on-premises** via Google Distributed Cloud: Dell-built 8-GPU appliance, confidential computing, air-gapped, with financial services as a named target (details in Q3). — [VentureBeat](https://venturebeat.com/technology/googles-gemini-can-now-run-on-a-single-air-gapped-server-and-vanish-when-you-pull-the-plug); [Dell blog](https://www.dell.com/en-us/blog/a-partnership-made-for-data-control/)
- **IBM watsonx.** Offered as SaaS or as software on Red Hat OpenShift on-premises or in private cloud, including air-gapped. — [IBM watsonx.ai](https://www.ibm.com/products/watsonx-ai)
  - Secondary sources describe routing most workloads to Granite 8B and reserving larger tiers for complex tasks, and cite IBM's existing presence in major banks. — [vdf.ai (secondary)](https://vdf.ai/blog/ibm-watsonx-vs-on-prem-ai-platforms/)
- **HPE / NVIDIA / Fortanix.** Turnkey on-prem "sovereign" agentic-AI platform in AI factories. — [Fortanix PR, Oct 2025](https://www.fortanix.com/company/pr/2025/10/fortanix-brings-secure-and-trusted-agentic-ai-to-cutting-edge-nvidia-confidential-computing-gpus)
- **Red Hat.** OpenShift sandboxed containers for private inference with proprietary LLMs, still upstream (details in Q3). — [Red Hat Emerging Technologies](https://next.redhat.com/2025/10/23/enhancing-ai-inference-security-with-confidential-computing-a-path-to-private-data-inference-with-proprietary-llms/)
- **Confidential SaaS as an alternative to on-prem.** Privatemode markets to financial services and European government, with a StGB §203 (professional secrecy) positioning and coding-tool integrations. — [privatemode.ai](https://www.privatemode.ai/); [Edgeless press release](https://www.edgeless.systems/press-release-edgeless-systems-nvidia-confidential-ai)
- **Research framing.** Hazy's hybrid-routing study found 60–80% energy, compute and cost reductions from routing queries local-first. — [Hazy retrospective](https://hazyresearch.stanford.edu/blog/2026-05-15-minions-to-openjarvis-retrospective)
- **Research-grade sensitivity routing:** PAPILLON / PUPA-SD (Q1). — [arXiv 2410.17127](https://arxiv.org/abs/2410.17127)

### Inferences
- In production, "routing by sensitivity" is regex/NER/DLP classification at a gateway. Its failure mode is exactly what the Q1 literature documents: untyped or quasi-identifying sensitive content passes through. For source code, the relevant detectors would be secrets scanners and code-ownership rules, not PII NER.
- Banks with the strictest constraints appear to prefer *moving the model in* over *filtering what goes out*: self-hosted Mistral at HSBC, on-prem Cohere at RBC, and air-gapped Gemini appliances. A confidential-computing appliance like GDC Gemini is the first way to get a *proprietary frontier* model on-prem while keeping its weights from the bank.

### Gaps
- No primary product documentation was found for **Dell AI Factory**, **HPE Private Cloud AI**, **Kong/Portkey/LiteLLM/Cloudflare** features that explicitly reroute to a local model by sensitivity class. Generic gateway blogs describe it, but I did not verify which products ship it natively. The search budget ran out.
- No bank publication was found that reports quality or leakage metrics for a hybrid router.
- Mistral's own on-prem offering (Mistral AI Studio / self-deployment) and other bank deployments of open-weight Llama, Qwen or DeepSeek (for example BNP Paribas, JPMorgan) were not verified in this pass.

## Q5. Quality gap between on-prem-runnable open-weight models and frontier models on coding benchmarks (2025–2026)

### Takeaway
On aggregate capability, Epoch measures open-weight models about **4 months / 8 ECI points behind** the closed frontier (May 2026), up from about 3.5 months (Oct 2025).

On agentic coding the gap is larger and depends on the benchmark:
- **Terminal-Bench 2.0** (Sept 2026 aggregate): best closed 82.7% (GPT-5.5) vs. best open-weight about 69% (GLM-5.1). DeepSeek-V4-Pro-Max is 67.9% and Kimi K2.6 66.7%.
- **SWE-bench Verified** is near saturation (about 80%) for both.
- **Standardized SWE-bench Pro** (Scale's public board) shows a large gap, but it evaluates older open checkpoints.

The strongest open models are also 100B–1T-scale MoE models that need a multi-GPU server, not a laptop. Hazy notes that coding and agentic tasks "still favor cloud."

### Cited Findings
- **Epoch AI (2026-05-29).** Since January 2026, the best open-weight models lag closed frontier models by an average of **4 months, or 8 ECI points**, which Epoch says is similar to the GPT-5 to GPT-5.5 gap. Top open: Kimi K2.6 (151.6 ECI), GLM-5 (146.6). Top closed: GPT-5.5 Pro (159.35), Gemini 3.5 Flash (156.31). The analysis does not isolate coding. — [Epoch AI](https://epoch.ai/data-insights/open-closed-eci-gap)
- **Epoch AI (2025-10-30).** Gap of 3.5 months (90% CI 1.1–5.3) and 7 ECI points (CI 0–14). Llama 3.1-405B briefly matched Claude 3.5 Sonnet until o1-mini was released. — [Epoch AI](https://epoch.ai/data-insights/open-weights-vs-closed-weights-models)
- Commentary on Epoch: closed labs keep their strongest internal models unreleased, and open models "hill-climb public benchmarks," so the real gap may exceed 4 months. — [TechJack Solutions (secondary)](https://techjacksolutions.com/ai-brief/epoch-ai-quantifies-the-open-vs-closed-ai-gap-4-months-8-eci/)
- **Terminal-Bench 2.0** (llm-stats aggregate, updated 2026-09-25):

  | Model | Score |
  |---|---|
  | GPT-5.5 | 82.7% |
  | Claude Mythos Preview | 82.0% |
  | Claude Sonnet 5 | 80.4% |
  | GPT-5.3 Codex | 77.3% |
  | Gemini 3.5 Flash | 76.2% |
  | GLM-5.1 (open weights) | 69.0% |
  | DeepSeek-V4-Pro-Max | 67.9% |
  | Kimi K2.6 | 66.7% |
  | Qwen3.6-27B | 59.3% |
  | MiniMax M2.7 | 57.0% |
  | GLM-5 | 56.2% |

  The aggregator's open-weight flags were inconsistent: it marked GLM-5 and Kimi K2.6 as closed, although Epoch lists both as open-weight. Scores mix vendor-reported and third-party runs. — [llm-stats Terminal-Bench 2.0](https://llm-stats.com/benchmarks/terminal-bench-2); [tbench.ai leaderboard](https://www.tbench.ai/leaderboard/terminal-bench/2.0)
- **Terminal-Bench 2.1** (Artificial Analysis, independent runs, pass@1 averaged over 3 repeats, 89 tasks): top is Claude Fable 5.1 at 91.4% (max effort). Open-weight scores were not visible in the fetched page. — [Artificial Analysis](https://artificialanalysis.ai/evaluations/terminalbench-2-1)
  - An aggregator snippet reports Qwen3.8-27B (dense, Apache 2.0) at 73.0% on TB 2.1. Not verified. — [llm-stats TB 2.1](https://llm-stats.com/benchmarks/terminal-bench-2.1)
- **SWE-bench Verified** (vendor-reported aggregate): DeepSeek-V4-Pro-Max 80.6%, tied with Gemini 3.1 Pro; MiniMax M3 80.5%; Qwen3.7 Max 80.4%; Kimi K2.6 80.2%. Near saturation. — [Morph (aggregator)](https://www.morphllm.com/best-open-source-coding-model-2026)
- **SWE-bench Pro, two sources that conflict sharply:**
  - Scale's standardized public leaderboard. Closed: Muse Spark 1.1 61.5%, GPT-5.4 (xHigh) 59.1%, Claude Opus 4.6 (thinking) 51.9%, Gemini 3.1 Pro (thinking) 46.1%. Open (older checkpoints): Qwen3-Coder-480B 38.7%, MiniMax-2.1 36.8%, Kimi-K2-instruct 27.7%, Qwen3-235B 21.4%, DeepSeek-v3.2 15.6%, GLM-4.6 9.7%. — [Scale Labs SWE-bench Pro public](https://labs.scale.com/leaderboard/swe_bench_pro_public)
  - Vendor-reported aggregate (as of 2026-09-14): Qwen3.8-Flash-Next 62.5%, GLM-5.2 62.1%, Qwen3.8-27B 61.7%, Kimi K2.6 58.6%. — [Morph SWE-bench Pro (aggregator)](https://www.morphllm.com/swe-bench-pro)
  - The gap between these reflects different scaffolds, vendor self-reporting, and Scale not having evaluated current open checkpoints.
- **Hazy Research (May 2026).** Chat and simple retrieval are "nearly saturated locally," but "browser-based agentic tasks and coding still favor cloud." Current MoE models also "waste resources on single-user local devices." — [Hazy retrospective](https://hazyresearch.stanford.edu/blog/2026-05-15-minions-to-openjarvis-retrospective)
- **Why the gap matters for hybrid design.** In Minions, the local model's weaknesses (multi-step instructions, long context) set the ceiling on hybrid quality. — [arXiv 2502.15964](https://arxiv.org/html/2502.15964v1)

### Inferences
- For a bank that wants frontier-quality agentic coding without sending code to a frontier provider, the best on-prem open-weight option in Sept 2026 is about **10–14 points behind on Terminal-Bench 2.0**. Roughly: GLM-5.1, DeepSeek-V4-Pro-Max and Kimi K2.6 at 67–69% versus about 80–83% for Claude and GPT. That is a real but not disqualifying gap.
- The models that reach those scores need datacenter-class multi-GPU servers, which an on-prem or confidential-GPU deployment can supply. Laptop-class models (about 27B dense) trail by another 10+ points on TB 2.0.
- SWE-bench Verified is too saturated to separate open from closed models. Terminal-Bench and standardized SWE-bench Pro are more informative, but open-model numbers there are often vendor-reported or stale, so a bank should run its own evaluation.
- Epoch's rolling 3–4 month lag means an on-prem open-weight deployment is roughly "last season's frontier." The key design question is whether the residual gap justifies a TEE-protected proprietary frontier model (Gemini on GDC, Apple-style PCC) versus a TEE-served or on-prem open-weight model.

### Gaps
- No independent, same-scaffold, current-checkpoint comparison of open-weight versus frontier models on SWE-bench Pro or Terminal-Bench 2.x was retrieved. Scale's public board lags on open models, and aggregators mix vendor claims.
- LiveCodeBench current standings were not retrieved.
- Parameter counts and minimum hardware for GLM-5.1, Kimi K2.6 and DeepSeek-V4 were not verified from primary model cards in this pass; the Morph page returned HTTP 429.
- Whether "Qwen3.7 Max/Plus" and "MiMo-V2.5-Pro" are open-weight is unclear. The aggregator marked them closed, and I could not verify licensing.
