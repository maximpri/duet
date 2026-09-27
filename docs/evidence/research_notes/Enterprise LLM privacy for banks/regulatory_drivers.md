# Regulatory and supervisory drivers for banks sending data to third-party AI/LLM providers (US, EU, UK, Singapore), as of September 2026

Note on sourcing: I verified items that came from 2025–2026 search and fetch results in this session. Items tagged **[KB]** come from my background knowledge of long-standing primary texts. They cite the canonical regulator URL, but I did not re-fetch them in this session. The report writer should treat [KB] items as high-confidence but unverified here. "In force", "guidance" and "proposed" status is marked throughout.

---

## Q1. EU: GDPR, pseudonymisation, EDPB AI opinion, AI Act, DORA, EBA/ECB outsourcing and cloud

### Takeaway
In the EU, five layers bind at once. GDPR sets minimisation, security (it names pseudonymisation and encryption) and transfer rules. The CJEU's Sept 2025 *SRB* judgment gives a legal benefit to pseudonymising data locally before it goes to a recipient that cannot re-identify it. DORA (in force since 17 Jan 2025) makes any LLM API an ICT third-party service: it needs a register entry, mandatory contract clauses, exit plans and concentration-risk analysis. The AI Act puts GPAI duties on model providers from Aug 2025. Its high-risk duties (credit scoring) were pushed back to 2 Dec 2027 by the AI Omnibus. The GDPR "Digital Omnibus" change to the definition of personal data is still only proposed, and the Council has resisted it.

### Cited Findings

**GDPR: core obligations [KB]**
- Art. 5(1)(c) data minimisation ("adequate, relevant and limited to what is necessary"). Art. 4(5) defines pseudonymisation: data can no longer be attributed to a specific person without additional information that is "kept separately" and protected by technical and organisational measures. Art. 25 (data protection by design) names pseudonymisation as an example measure. Art. 32 (security) lists "the pseudonymisation and encryption of personal data". Art. 28 requires a processor contract (DPA). Art. 30 requires records of processing. Arts. 44–49 govern transfers to third countries. Recital 26: pseudonymised data remain personal data; anonymous data are out of scope. — [GDPR, EUR-Lex](https://eur-lex.europa.eu/eli/reg/2016/679/oj)
- EDPB Recommendations 01/2020 on supplementary transfer measures (final version June 2021):
  - Where a third-country importer needs data in the clear, and public-authority access there goes beyond what is necessary, the EDPB found no effective technical supplementary measure. This covers cloud processing of clear data (Use Case 6) and remote access for business purposes (Use Case 7).
  - Transferring properly pseudonymised data (Use Case 2), and encrypted storage where the keys stay in the EEA (Use Case 1), can be effective.
  - This is the textbook reason why "LLM inference needs plaintext" matters for transfers. — [EDPB Recommendations 01/2020](https://www.edpb.europa.eu/our-work-tools/our-documents/recommendations/recommendations-012020-measures-supplement-transfer_en) [KB]
- EU–US Data Privacy Framework adequacy decision, 10 July 2023: transfers to DPF-certified US providers need no SCCs or supplementary measures. — [European Commission press release IP/23/3721](https://ec.europa.eu/commission/presscorner/detail/en/ip_23_3721) [KB]. The General Court dismissed the *Latombe* challenge (T-553/23) in Sept 2025, and an appeal to the CJEU is reported to be pending. — [CURIA case T-553/23](https://curia.europa.eu/juris/liste.jsf?num=T-553/23) [KB; appeal status not verified this session]

**Pseudonymisation: EDPB Guidelines 01/2025 and the CJEU SRB judgment**
- EDPB Guidelines 01/2025 on Pseudonymisation:
  - Adopted 16 Jan 2025 for public consultation (closed 28 Feb 2025).
  - Pseudonymised data remain personal data. For pseudonymisation to count, the attributing information must be held separately and securely.
  - The guidelines set out the legal and technical requirements for effective pseudonymisation and include 10 worked examples.
  - Pseudonymisation is presented as supporting legitimate-interest balancing, compatibility of further processing, and Arts. 25/32.
  - Sources: [EDPB consultation page](https://www.edpb.europa.eu/our-work-tools/documents/public-consultations/2025/guidelines-012025-pseudonymisation_en); [EDPB guidelines PDF](https://www.edpb.europa.eu/system/files/2025-01/edpb_guidelines_202501_pseudonymisation_en.pdf); [Hunton summary](https://www.hunton.com/insights/publications/edpb-advises-on-pseudonymisation-for-gdpr-compliance); [Bird & Bird](https://www.twobirds.com/en/insights/2025/'what's-in-a-name'-edpb-publishes-draft-guidelines-on-pseudonymisation)
- CJEU *EDPS v SRB* (C-413/23 P), 4 Sept 2025:
  - Facts: the Single Resolution Board pseudonymised comments from Banco Popular shareholders and sent them to Deloitte.
  - Pseudonymised data are not personal data "in all cases and for every person". For a recipient that cannot reverse the pseudonymisation and cannot identify people by other means, they may not be personal data.
  - They remain personal data for the original controller.
  - The controller's duty to inform data subjects is judged from the controller's side at the time of collection, whatever happens after the transfer.
  - Sources: [CURIA press release](https://curia.europa.eu/site/upload/docs/application/pdf/2025-09/cp250107en.pdf); [Covington Inside Privacy](https://www.insideprivacy.com/eu-data-protection/eu-court-of-justice-clarifies-the-concept-of-personal-data-in-the-context-of-a-transfer-of-pseudonymized-data-to-third-parties/); [FPF analysis](https://fpf.org/blog/rethinking-personal-data-the-cjeus-contextual-turn-in-edps-vs-srb/); [Skadden](https://www.skadden.com/insights/publications/2025/11/in-a-landmark-decision-eu-court-clarifies)
- GDPR "Digital Omnibus" (Commission proposal, Nov 2025), status **proposed, not adopted**:
  - The proposal would write the SRB relative approach into the definition of personal data: pseudonymised data would not be personal data for an entity with no means to re-identify.
  - It would also let the Commission decide by implementing acts when data stop being personal after pseudonymisation.
  - The EDPB and EDPS objected that this "would result in significantly narrowing the concept of personal data".
  - A leaked Council compromise (spring 2026) **removes** the new definition and the Commission's implementing powers on pseudonymisation.
  - Sources: [IAPP on Council compromise](https://iapp.org/news/a/eu-member-states-leaked-digital-omnibus-compromise-proposal-eliminates-revised-gdpr-definition-of-personal-data); [EDRi analysis of Council text](https://edri.org/wp-content/uploads/2026/03/EDRi-Data-Omnibus-Analysis-Council-Compromise-text.pdf); [EP Legislative Train](https://www.europarl.europa.eu/legislative-train/theme-a-new-plan-for-europe-s-sustainable-prosperity-and-competitiveness/file-digital-package); [Hogan Lovells](https://www.hoganlovells.com/en/publications/eu-digital-omnibus-where-simplification-is-likely-and-what-businesses-should-plan-for)

**EDPB Opinion 28/2024 on AI models (adopted 17 Dec 2024) [KB]**
- An AI model trained on personal data cannot be assumed anonymous. Anonymity is judged case by case: both the chance of extracting personal data from the model and the chance of getting it out through queries must be "insignificant".
- It sets out a three-step legitimate-interest test for developing and deploying models.
- If a model was developed unlawfully, that can affect whether deploying it is lawful. A deploying controller should carry out an appropriate assessment of whether the model was developed lawfully. — [EDPB Opinion 28/2024](https://www.edpb.europa.eu/our-work-tools/our-documents/opinion-board-art-64/opinion-282024-certain-data-protection-aspects_en)

**EU AI Act (Reg. (EU) 2024/1689) and the AI Omnibus**
- Timeline:
  - Entered into force 1 Aug 2024.
  - Prohibitions and AI literacy (Art. 4) apply from 2 Feb 2025.
  - GPAI model-provider obligations (Arts. 53–55: technical documentation, copyright policy, training-data summary, plus systemic-risk duties) apply from 2 Aug 2025.
  - GPAI models placed on the market before then have until 2 Aug 2027.
  - Source: [AI Act, EUR-Lex](https://eur-lex.europa.eu/eli/reg/2024/1689/oj) [KB]
- Annex III point 5(b) classes AI used to assess creditworthiness or set credit scores of natural persons as high-risk (fraud detection is excluded). Point 5(c) does the same for life and health insurance risk assessment and pricing. — [AI Act](https://eur-lex.europa.eu/eli/reg/2024/1689/oj) [KB]
- Deployer duties for high-risk systems:
  - Art. 26: human oversight, relevance of input data, and keeping automatically generated logs for at least 6 months.
  - For financial institutions, some deployer duties (monitoring, log-keeping) are treated as met through their existing financial-services governance rules.
  - Art. 74(6): financial supervisors are the market-surveillance authorities for high-risk AI placed or used by regulated financial institutions.
  - Source: [AI Act](https://eur-lex.europa.eu/eli/reg/2024/1689/oj) [KB]
- AI Omnibus, now **law**:
  - Provisional political agreement 7 May 2026.
  - Published as Regulation (EU) 2026/1744 in the OJ on 24 July 2026; entered into force 27 July 2026.
  - Stand-alone Annex III high-risk obligations, including credit scoring, move from 2 Aug 2026 to **2 Dec 2027**. Annex I (product-embedded) obligations move to **2 Aug 2028**.
  - Sources: [CSA research note](https://labs.cloudsecurityalliance.org/research/csa-research-note-eu-ai-act-high-risk-deadline-omnibus-20260/); [Gibson Dunn](https://www.gibsondunn.com/eu-ai-act-omnibus-agreement-postponed-high-risk-deadlines-and-other-key-changes/); [Pinsent Masons](https://www.pinsentmasons.com/out-law/news/rules-high-risk-ai-delayed-under-eu-omnibus-deal)
- Other Omnibus changes (per Gibson Dunn's summary of the agreement):
  - GPAI obligations and timelines are **unchanged** (applicable since 2 Aug 2025).
  - Art. 4 AI literacy is softened to "support" literacy rather than guarantee a level.
  - Processing special-category data for bias detection is extended to all AI systems and GPAI models, under strict necessity.
  - Art. 50 transparency still applies from 2 Aug 2026, with a watermarking grace period to 2 Dec 2026 for systems already on the market. — [Gibson Dunn](https://www.gibsondunn.com/eu-ai-act-omnibus-agreement-postponed-high-risk-deadlines-and-other-key-changes/)

**DORA (Reg. (EU) 2022/2554), applicable from 17 Jan 2025 [KB unless noted]**
- Art. 28: financial entities stay fully responsible for outsourced ICT. They must keep a **register of information** on all ICT third-party contracts, assess the provider before contracting, analyse concentration risk, and have exit strategies for ICT services that support critical or important functions. — [DORA, EUR-Lex](https://eur-lex.europa.eu/eli/reg/2022/2554/oj)
- Art. 30 mandatory contract terms. For all ICT contracts:
  - where data will be processed (regions or countries);
  - data protection covering availability, authenticity, integrity and **confidentiality**;
  - access to, recovery of and return of data;
  - cooperation with competent authorities.
  - For critical or important functions, also: full audit, access and inspection rights; exit and transition periods; participation in TLPT.
  - Source: [DORA](https://eur-lex.europa.eu/eli/reg/2022/2554/oj)
- Art. 9: ICT security policies and tools must prevent breaches of confidentiality and data loss (in effect, data-leakage prevention). — [DORA](https://eur-lex.europa.eu/eli/reg/2022/2554/oj)
- Critical ICT third-party providers (CTPPs):
  - On 18 Nov 2025 the ESAs designated **19 CTPPs** for direct EU oversight. They were identified from firms' Registers of Information plus a criticality assessment.
  - The list covers hyperscale cloud, data-centre, infrastructure and network providers, and FS-specific technology vendors. Google Cloud, Microsoft and AWS are on it.
  - Sources: [EBA press release](https://www.eba.europa.eu/publications-and-media/press-releases/european-supervisory-authorities-designate-critical-ict-third-party-providers-under-digital); [PwC Legal](https://legal.pwc.de/en/news/articles/esas-publish-first-list-of-critical-ict-third-party-providers-under-dora); [Morgan Lewis](https://www.morganlewis.com/blogs/sourcingatmorganlewis/2025/11/dora-eu-regulators-announce-list-of-critical-ict-third-party-providers)
  - An aggregator list shows **no pure-play foundation-model developer** (OpenAI, Anthropic, Mistral) among the 19. However, that list's names do not fully match other reports, so treat specific names beyond AWS, Google and Microsoft as unverified. — [regulation-dora.eu (secondary)](https://www.regulation-dora.eu/blog/critical-ict-third-party-designations-october-2025). The ESAs update the list annually. — same source

**ECB and EBA outsourcing and cloud guidance**
- ECB Guide on outsourcing cloud services to cloud service providers, final 16 July 2025, **non-binding**:
  - It turns DORA and CRD third-party rules into expected practice.
  - Exit plans: detailed, costed, per service, for cloud services supporting critical or important functions, drawn up before go-live, with a list of qualified alternative providers.
  - Data: encryption in transit, at rest and "where feasible, in use"; data residency; asset classification and inventory; identity and access management.
  - Sources: [ECB Guide PDF](https://www.bankingsupervision.europa.eu/ecb/pub/pdf/ssm.supervisory_guides202507.en.pdf); [Jones Day](https://www.jonesday.com/en/insights/2025/11/new-ecb-guide-on-outsourcing-cloud-services-to-cloud-service-providers); [CMS](https://cms-lawnow.com/en/ealerts/2025/08/ecb-issues-new-guidance-on-outsourcing-cloud-services)
- EBA Guidelines on outsourcing arrangements (EBA/GL/2019/02, 2019): risk-based data-location and data-security assessment, audit and access rights, exit strategies, and notifying material outsourcing. DORA is now lex specialis for ICT. — [EBA outsourcing guidelines](https://www.eba.europa.eu/regulation-and-policy/internal-governance/guidelines-on-outsourcing-arrangements) [KB]

### Inferences
- *SRB* plus the EDPB pseudonymisation guidelines change the economics of redaction. Suppose the bank pseudonymises locally, keeps the re-identification key on-prem, and the LLM vendor cannot re-identify. Then the vendor-side data may fall outside GDPR from the vendor's perspective. That weakens transfer and processor-chain concerns, but it depends on the pseudonymisation being strong enough to survive the "means reasonably likely" test. The bank's own GDPR duties do not go away, and the Omnibus attempt to codify this is stalled in the Council. A coding agent that pseudonymises reliably and keeps the key local therefore buys real legal headroom, but only if it can **prove** re-identification is infeasible, which needs audit evidence.
- Every LLM API used for a critical or important function counts as a DORA ICT third-party arrangement, with a register entry, Art. 30 clauses, an exit plan and concentration analysis. Frontier-model vendors are not CTPPs, so there is no EU direct oversight of them to lean on. The bank carries the full due-diligence and audit burden. On-prem or local models largely avoid this: they are either in-house ICT or covered under existing hyperscaler contracts.
- For a coding assistant, the AI Act adds little direct burden on the bank: coding is not an Annex III use, and the GPAI duties fall on the model provider. The binding constraints are GDPR (personal data in code, fixtures and logs) and DORA (source code is an ICT asset; confidentiality; exit).

### Gaps
- Whether a **final** version of EDPB Guidelines 01/2025 was adopted after the SRB judgment: not confirmed in this session.
- The final outcome of the GDPR Digital Omnibus (trilogue status as of Sept 2026): not confirmed.
- The official ESAs CTPP PDF was not fetched, so the full list of 19 names is unverified.
- Whether EBA has formally revised the 2019 outsourcing guidelines to align with DORA: not verified.
- Whether the Latombe appeal before the CJEU has been decided: not verified.

---

## Q2. United States: GLBA, TPRM, model risk (SR 11-7 successor), OCC/Fed/FDIC on AI, FFIEC, NYDFS Part 500 and 2024 AI guidance, SEC, state laws, Treasury 2024, 2025–26 changes

### Takeaway
US banking law has no AI-specific data-egress rule. The obligations come from GLBA safeguards (service-provider oversight, access control, logging, disposal), the 2023 interagency TPRM guidance, NYDFS Part 500 (asset inventory including AI, MFA, monitoring) and SEC Reg S-P (72-hour service-provider breach notice). The biggest 2026 change is deregulatory. On 17 Apr 2026 the OCC, Fed and FDIC **rescinded SR 11-7 / OCC 2011-12**. The replacement is principles-based, non-binding MRM guidance aimed at banks over $30bn, and it **explicitly excludes generative and agentic AI**. An AI-specific RFI has been signalled. So GenAI data-handling expectations are currently driven by security, privacy and TPRM rules, not model-risk rules.

### Cited Findings

**Model risk management: SR 11-7 rescinded (April 2026)**
- What was issued (17 Apr 2026, OCC, Fed and FDIC):
  - Revised interagency MRM guidance: OCC Bulletin 2026-13; issued by the Fed as SR 26-2.
  - It rescinds OCC Bulletin 2011-12 (SR 11-7 equivalent), OCC 1997-24 (credit scoring models), OCC 2021-19 (BSA/AML model risk statement) and the Comptroller's Handbook MRM booklet.
  - A model is "a complex quantitative method, system, or approach that applies statistical, economic, or financial theories to process input data into quantitative estimates."
  - The guidance is non-binding: "non-compliance with this guidance will not result in supervisory criticism."
  - Sources: [OCC Bulletin 2026-13](https://www.occ.gov/news-issuances/bulletins/2026/bulletin-2026-13.html); [OCC NR 2026-29](https://www.occ.gov/news-issuances/news-releases/2026/nr-occ-2026-29.html); [Sullivan & Cromwell](https://www.sullcrom.com/insights/memo/2026/April/OCC-Fed-FDIC-Issue-Revised-Guidance-Model-Risk-Management); [Orrick](https://www.orrick.com/en/Insights/2026/04/Agencies-Overhaul-Model-Risk-Management-Guidance-for-Banks-Heres-What-Changed)
- Explicit carve-out: "Generative AI and agentic AI models are novel and rapidly evolving. As such, they are not within the scope of this guidance." The agencies say future AI guidance is planned. The new guidance targets banks over $30bn in total assets. — [OCC Bulletin 2026-13](https://www.occ.gov/news-issuances/bulletins/2026/bulletin-2026-13.html); [Sullivan & Cromwell](https://www.sullcrom.com/insights/memo/2026/April/OCC-Fed-FDIC-Issue-Revised-Guidance-Model-Risk-Management)
- The agencies plan an RFI on model risk management and on banks' use of AI, including generative and agentic AI. Fed Vice Chair for Supervision Bowman has called for an assessment of whether AI supervisory guidance is "fit for the future". — [Tandem May 2026 roundup (secondary)](https://tandem.app/blog/what-are-the-regulators-saying-about-artificial-intelligence-ai-may-update-2026); [Consumer Finance Insights](https://www.consumerfinanceinsights.com/2026/05/19/4745/)

**Third-party risk management**
- Interagency Guidance on Third-Party Relationships: Risk Management (Fed, FDIC, OCC; final 6 June 2023; Fed SR 23-4). It is lifecycle-based: planning, due diligence, contract negotiation, ongoing monitoring and termination. Contract terms should cover confidentiality, data ownership and use, audit and access rights, subcontracting, and breach notification. Using a third party "does not diminish" the bank's responsibility. — [Federal Reserve press release](https://www.federalreserve.gov/newsevents/pressreleases/bcreg20230606a.htm) [KB]

**GLBA safeguards**
- Banks are covered by the Interagency Guidelines Establishing Information Security Standards (e.g. 12 CFR 364 App. B). These require a risk-based program, access controls and encryption. Service providers must be overseen through due diligence, **contracts requiring appropriate safeguards**, and monitoring. — [eCFR 12 CFR 364 App. B](https://www.ecfr.gov/current/title-12/chapter-III/subchapter-B/part-364/appendix-Appendix%20B%20to%20Part%20364) [KB]
- Non-bank financial institutions are covered by the FTC Safeguards Rule (16 CFR 314):
  - Amended rule effective June 2023; FTC breach notification (500+ consumers) effective 13 May 2024.
  - Specific requirements: access controls, a data and systems inventory, encryption, MFA, secure disposal no later than 2 years after last use (unless needed), change management, monitoring and **logging of authorized users' activity**, and service-provider oversight (§314.4(f)).
  - Source: [FTC Safeguards Rule](https://www.ftc.gov/legal-library/browse/rules/safeguards-rule) [KB]

**NYDFS Part 500 and the AI cybersecurity letter**
- NYDFS industry letter, 16 Oct 2024, "Cybersecurity Risks Arising from AI and Strategies to Combat Related Risks". It is **guidance that interprets existing Part 500**, not a new rule. It flags:
  - "Products that use AI typically require the collection and processing of substantial amounts of data, often including NPI."
  - Supply-chain and vendor dependence ("each link in this supply chain introduces potential security vulnerabilities").
  - Source: [NYDFS industry letter](https://www.dfs.ny.gov/industry-guidance/industry-letters/il20241016-cyber-risks-ai-and-strategies-combat-related-risks)
- Controls the letter expects under Part 500:
  - Due diligence on AI vendors, and "consider incorporating additional representations and warranties related to the secure use of Covered Entities' NPI, including requirements to take advantage of available enhanced privacy, security, and confidentiality options."
  - MFA for all authorized users by Nov 2025.
  - Disposal of NPI no longer necessary.
  - Data and asset inventories that include AI systems from 1 Nov 2025.
  - Monitoring for "unusual query behaviors that might indicate an attempt to extract NPI."
  - Source: [NYDFS industry letter](https://www.dfs.ny.gov/industry-guidance/industry-letters/il20241016-cyber-risks-ai-and-strategies-combat-related-risks)
- Part 500 as amended 1 Nov 2023 (phased to Nov 2025) is **binding**: §500.11 third-party service provider security policy, §500.7 access privileges, §500.6 audit trail, §500.13 asset inventory, §500.12 MFA, §500.14 monitoring and training. — [NYDFS cybersecurity resource center](https://www.dfs.ny.gov/industry_guidance/cybersecurity) [KB]

**SEC and FINRA**
- Amended Reg S-P (adopted May 2024):
  - Compliance by 3 Dec 2025 for larger entities and 3 Jun 2026 for smaller ones.
  - Requires an incident response program, customer breach notification, **service-provider oversight** through due diligence and monitoring, and service providers notifying the firm **within 72 hours** of a breach of a customer information system. Recordkeeping is also required.
  - Sources: [Holland & Knight](https://www.hklaw.com/en/insights/publications/2026/05/regulation-s-p-amendments-compliance-deadline-approaching); [FINRA reminder](https://www.finra.org/rules-guidance/guidance/sec-regulation-s-p-compliance-date-reminder-20251114); [Foley](https://www.foley.com/insights/publications/2025/12/amended-regulation-s-p-here-to-stay-and-being-examined-in-2026/)
- FINRA 2026 Annual Regulatory Oversight Report (9 Dec 2025) has a GenAI section flagging agent "data sensitivity". Agents working on sensitive data "may unintentionally store, explore, disclose, or misuse sensitive or proprietary information." Suggested controls: tracking agent actions, restricting system access, and testing for privacy issues. The report creates no new obligations. — [FINRA 2026 report PDF](https://www.finra.org/sites/default/files/2025-12/2026-annual-regulatory-oversight-report.pdf); [Debevoise](https://www.debevoise.com/insights/publications/2025/12/finras-2026-regulatory-oversight-report-continued)
- FINRA Regulatory Notice 24-09 (June 2024): existing rules, including supervision, recordkeeping and Reg S-P, apply to GenAI in a technology-neutral way. — [FINRA RN 24-09](https://www.finra.org/rules-guidance/notices/24-09) [KB]

**Treasury, and the 2025–26 policy direction**
- Treasury report "Uses, Opportunities, and Risks of AI in the Financial Services Sector", 19 Dec 2024 (summarising the June 2024 RFI):
  - Respondents flagged data privacy, poisoning and breaches.
  - They flagged concentration risk: a few firms dominate advanced models.
  - They proposed privacy-enhancing technologies (homomorphic encryption, federated learning).
  - The report calls for consistent definitions, data-privacy clarity and third-party oversight.
  - Sources: [Treasury press release](https://home.treasury.gov/news/press-releases/jy2760); [Debevoise summary](https://www.debevoisedatablog.com/2025/01/23/treasurys-post-2024-rfi-report-on-ai-in-financial-services-uses-opportunities-and-risks/); [NYC Bar](https://www.nycbar.org/reports/reflections-on-us-treasury-department-report-on-ai-in-financial-services/)
- The new administration's direction is deregulatory at federal level and pre-emptive towards states:
  - The 11 Dec 2025 Executive Order "Ensuring A National Policy Framework For Artificial Intelligence" directs a DOJ AI Litigation Task Force to challenge state AI laws and names Colorado's AI Act.
  - Colorado delayed its AI Act first to 30 Jun 2026. SB 189 (signed 14 May 2026) then repealed and replaced it with a narrower ADMT disclosure law effective 1 Jan 2027.
  - Sources: [Clark Hill](https://www.clarkhill.com/news-events/news/what-does-trumps-ai-executive-order-mean-for-colorados-ai-act/); [Skadden](https://www.skadden.com/insights/publications/2026/06/colorado-repeals-and-replaces-its-ai-act); [Cooley state AI laws tracker](https://www.cooley.com/news/insight/2026/2026-04-24-state-ai-laws-where-are-they-now)
- State comprehensive privacy laws (CCPA/CPRA and others) generally exempt GLBA-covered data or institutions at entity or data level. The CCPA's exemption is data-level, so non-GLBA data such as employee data and some marketing data remains in scope. — [California AG CCPA page](https://oag.ca.gov/privacy/ccpa) [KB]
- FFIEC: the IT Examination Handbook (Architecture, Infrastructure & Operations; Outsourcing Technology Services; Information Security) remains the examiner baseline for vendor and cloud reliance. — [FFIEC IT Handbook](https://ithandbook.ffiec.gov/) [KB]

### Inferences
- For a US bank, a GenAI coding assistant is currently **outside formal model-risk guidance**, because the 2026 guidance carves GenAI out. That shifts scrutiny to information-security and TPRM examiners. What they will ask for is evidence of access control, logging, data inventory (NYDFS §500.13 now explicitly includes AI systems), NPI minimisation and vendor oversight.
- The NYDFS letter is notable because it **endorses the contractual route**: it tells firms to require vendors to "take advantage of available enhanced privacy, security, and confidentiality options", which is effectively a zero-data-retention or enterprise tier. It also asks for **technical monitoring** of query behaviour. A local, logging proxy satisfies both.
- Reg S-P's 72-hour service-provider notification assumes the vendor can detect and report a breach. If NPI never reaches the vendor, the service-provider oversight burden for that flow shrinks.

### Gaps
- The text and date of the promised OCC/Fed/FDIC AI RFI: not confirmed as issued by Sept 2026.
- Any OCC, Fed or FDIC statement specifically on GenAI data leakage: none found.
- The SEC's June 2025 withdrawal of the predictive-data-analytics conflicts proposal was not verified in this session.
- FFIEC CAT sunset details were not verified.

---

## Q3. PCI DSS v4.0/4.0.1: card data in LLM prompts

### Takeaway
PCI DSS has no AI carve-out. Any system that stores, processes or transmits cardholder data, or can affect its security, is in scope, and an LLM endpoint that receives a PAN is included. The PCI SSC's Sept 2025 AI Principles (guidance) explicitly say to limit sensitive data given to AI, sanitise it, use tokens or single-use PANs, and log prompts and AI actions. That points strongly to tokenisation or redaction before any prompt leaves the cardholder data environment (CDE).

### Cited Findings
- PCI SSC "AI Principles: Securing the Use of AI in Payment Environments", 11 Sept 2025, **guidance, not new requirements**:
  - "AI systems must be deployed and managed in compliance with applicable PCI SSC requirements."
  - "AI systems should not be trusted with high-impact secrets or unprotected sensitive data." Limit "the sensitive data provided to the AI system in the first place." Training data should be "sanitised of sensitive information and secrets prior to use."
  - Where payment data is needed, consider "payment tokens or single-use PANs to limit the scope and impact". Requirement 3 (stored data) and Requirement 4 (transmission) apply to AI systems.
  - Actions "can be logged and monitored", with logging of "prompt inputs and reasoning process", and "a (human) individual held responsible".
  - Use "limited, use case, and context specific credentials". Keep a human in the loop for deployment chains. Protect against prompt injection.
  - Source: [PCI SSC blog](https://blog.pcisecuritystandards.org/ai-principles-securing-the-use-of-ai-in-payment-environments)
- PCI SSC also issued guidance (spring 2025) on assessors using AI in PCI assessments. AI may help with document review and reports, but human assessors must lead, and using AI "does not remove or bypass the need to meet any applicable requirement." — [PCI SSC blog](https://blog.pcisecuritystandards.org/new-guidance-integrating-artificial-intelligence-into-pci-assessments)
- PCI DSS v4.0.1 (June 2024) is the current version. v4.0 was retired 31 Dec 2024, and the future-dated v4 requirements became mandatory 31 Mar 2025. Key requirements:
  - Req. 3: no storage of sensitive authentication data after authorisation; PAN rendered unreadable wherever stored, logs included.
  - Req. 10: audit logs.
  - Req. 12.5.2: annual scope confirmation.
  - Req. 12.8: third-party service provider management (list of TPSPs, written acknowledgement of responsibility, monitoring of compliance status).
  - Req. 12.10.7: incident response when PAN is found where not expected.
  - Source: [PCI SSC Document Library](https://www.pcisecuritystandards.org/document_library/) [KB]

### Inferences
- A PAN pasted into a prompt, or embedded in code, test fixtures or logs that an agent sends to a vendor LLM, would:
  1. bring the vendor endpoint and its logs into PCI scope as a TPSP under 12.8;
  2. create unexpected PAN storage in vendor and local logs, triggering 12.10.7 incident procedures;
  3. be hard to remediate if the vendor retains prompts.
- Few LLM vendors offer PCI DSS validation for inference endpoints (not verified here). So the practical path is detecting and tokenising PAN, SAD and secrets **before** egress, and keeping audit logs of what was redacted. That is a technical control, not a contractual one.

### Gaps
- Whether any major LLM API vendor holds a PCI DSS attestation of compliance (AOC) that covers its inference endpoints: not researched here.
- No PCI SSC FAQ specifically on LLM prompt logs was found.

---

## Q4. UK: PRA SS2/21, SS1/23, operational resilience, CTP regime, FCA/BoE AI work, ICO

### Takeaway
The UK regulators say they will supervise AI through existing technology-neutral rules, with no AI-specific rulebook. The FCA confirmed this in January 2026 and the BoE/PRA in April 2026. The binding drivers are:
- SS2/21 outsourcing and third-party risk (data location and security, audit rights, exit);
- SS1/23 model risk, whose broad model definition covers AI including vendor models;
- operational resilience impact tolerances (from 31 Mar 2025);
- UK GDPR as interpreted by the ICO.

The Critical Third Parties regime went live with the first designations on 10 July 2026: AWS, Google Cloud, Microsoft and Oracle. There are no AI model vendors yet, although Parliament's Treasury Committee urged designating major AI providers.

### Cited Findings
- **PRA SS2/21** "Outsourcing and third party risk management" (Mar 2021, effective 31 Mar 2022) covers both material outsourcing and other third-party arrangements:
  - a risk-based approach to data location and data security, with confidentiality and access controls;
  - audit, access and information rights;
  - business continuity and exit plans (stressed exit for material outsourcing);
  - notification of material outsourcing to the PRA.
  - Source: [PRA SS2/21](https://www.bankofengland.co.uk/prudential-regulation/publication/2021/march/outsourcing-and-third-party-risk-management-ss) [KB]
- **PRA SS1/23** "Model risk management principles for banks" (published 17 May 2023, effective 17 May 2024, for banks with internal-model approval): a broad model definition covering AI/ML; third-party and vendor models held to the same standards; a model inventory and risk tiering. — [PRA SS1/23](https://www.bankofengland.co.uk/prudential-regulation/publication/2023/may/model-risk-management-principles-for-banks-ss) [KB]. Industry told the PRA in Feb 2026 it doubts "traditional model risk management and validation approaches can scale effectively" to generative and agentic AI. — [Covington Global Policy Watch](https://www.globalpolicywatch.com/2026/04/uk-financial-services-regulators-approach-to-artificial-intelligence-in-2026/)
- **Operational resilience** (PRA PS21/3, SS1/21; FCA PS21/3): firms had to be able to stay within impact tolerances for important business services by 31 Mar 2025, including those that depend on third parties. — [PRA operational resilience](https://www.bankofengland.co.uk/prudential-regulation/publication/2021/march/operational-resilience-impact-tolerances-for-important-business-services) [KB]
- **Critical Third Parties regime** (FSMA 2023):
  - HM Treasury made the first four designations on 10 July 2026: Amazon Web Services EMEA SARL, Google Cloud EMEA Limited, Microsoft Ireland Operations Ltd and Oracle Corporation UK Limited.
  - BoE, PRA and FCA oversight began 13 July 2026. It is a "rolling programme", with more designations expected.
  - Sources: [BoE news, July 2026](https://www.bankofengland.co.uk/news/2026/july/uk-financial-regulators-to-begin-overseeing-critical-third-parties-announced-by-hmt); [HSF Kramer](https://www.hsfkramer.com/notes/fsrandcorpcrime/2026-posts/hm-treasury-makes-first-designations-under-uk-ctp-regime); [MoFo](https://www.mofo.com/resources/insights/260716-uk-announces-list-of-first-critical-third-parties)
  - The CTP regime does not relieve firms of their own SS2/21 duties. — [BoE/PRA/FCA joint foreword, Nov 2024](https://www.bankofengland.co.uk/prudential-regulation/publication/2024/november/joint-foreword-critical-third-parties-to-the-uk-financial-sector)
- **BoE/FCA third AI survey** (21 Nov 2024):
  - 75% of firms use AI and 10% plan to within 3 years.
  - One-third of use cases are third-party implementations, up from 17% in 2022.
  - The top 3 providers account for 73% of cloud, 44% of model and 33% of data provision.
  - Foundation models are 17% of use cases.
  - 46% of firms report only "partial understanding" of the AI they use, partly because of third-party models.
  - Data privacy and protection is the top perceived risk category, and data-protection regulation is the **largest perceived regulatory constraint**.
  - Source: [BoE/FCA AI survey 2024](https://www.bankofengland.co.uk/report/2024/artificial-intelligence-in-uk-financial-services-2024)
- **FCA and policy developments, 2025–26:**
  - FCA AI Live Testing (first cohort Oct 2025).
  - Supercharged Sandbox with synthetic data.
  - FCA confirmed on 27 Jan 2026 that it "does not currently plan to introduce AI-specific rules".
  - Mills Review of AI in retail FS launched 27 Jan 2026.
  - Source: [Covington Global Policy Watch](https://www.globalpolicywatch.com/2026/04/uk-financial-services-regulators-approach-to-artificial-intelligence-in-2026/)
- **Treasury Committee report** on AI in FS (20 Jan 2026):
  - Warned that the regulators' "wait-and-see" approach "risks serious harm".
  - Recommended AI-specific stress testing, guidance by end-2026, and designating major **AI and cloud providers** as CTPs.
  - BoE/PRA responded on 1 Apr 2026, keeping "a technology-agnostic approach".
  - Sources: [Covington](https://www.globalpolicywatch.com/2026/04/uk-financial-services-regulators-approach-to-artificial-intelligence-in-2026/); [BoE response letter](https://www.bankofengland.co.uk/-/media/boe/files/letter/2026/response-to-tsc-inquiry-report-on-ai-in-financial-services); [Parliament report](https://publications.parliament.uk/pa/cm5901/cmselect/cmtreasy/684/report.html)
- **ICO** "Guidance on AI and data protection" (updated Mar 2023) covers:
  - accountability and DPIAs;
  - controller and processor roles in AI supply chains;
  - security risks specific to AI, including model inversion and membership inference;
  - data minimisation techniques, including pseudonymisation and privacy-enhancing technologies;
  - the statistical accuracy of AI outputs.
  - Source: [ICO AI guidance](https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/artificial-intelligence/guidance-on-ai-and-data-protection/) [KB]

### Inferences
- UK firms face the same shape of obligation as under EU DORA/EBA rules, but less prescriptively. Frontier LLM API vendors are neither CTPs nor, in most cases, "material outsourcing" for a coding tool. The bank therefore does its own SS2/21 due diligence. The concentration statistics (44% of model provision with the top three) feed directly into Treasury Committee pressure and possible future designations.
- The survey result that data protection is the largest perceived regulatory constraint on AI is direct evidence of demand for local and redacted processing among UK FS firms.

### Gaps
- Whether the ICO issued final generative-AI guidance, or updated its AI guidance for the Data (Use and Access) Act 2025, was not verified in this session.
- No UK regulator publication specifically on GenAI prompt data leakage was found.
- The fourth BoE/FCA AI survey results were not yet found.

---

## Q5. Singapore: MAS TRM, FEAT, 2024 AI MRM information paper, 2025 AI Risk Management Guidelines, MindForge

### Takeaway
Singapore stacks five things:
1. binding MAS technology-risk and outsourcing Notices;
2. strict banking secrecy (Banking Act s.47), which limits disclosure of customer information to third parties;
3. the PDPA;
4. a maturing AI-specific layer: the Dec 2024 AI model risk information paper, then the Nov 2025 proposed Guidelines on AI Risk Management, which explicitly cover GenAI and AI agents but were **not yet final** at Sept 2026 and come with a 12-month transition;
5. Project MindForge operational handbooks, from Nov 2025 and Jan 2026.

### Cited Findings
- **MAS Technology Risk Management Guidelines** (Jan 2021, guidelines): IT third-party risk (including cloud), data and infrastructure security including data loss prevention, access control, and logging and monitoring. — [MAS TRM Guidelines](https://www.mas.gov.sg/regulation/guidelines/technology-risk-management-guidelines) [KB]
- **Banking Act s.47** (banking secrecy): customer information may be disclosed only under the Third Schedule conditions, which include conditional disclosure for outsourcing. This is an absolute statutory gate beyond PDPA. — [Banking Act 1970, SSO](https://sso.agc.gov.sg/Act/BA1970) [KB]
- **FEAT Principles** (Nov 2018): fairness, ethics, accountability and transparency in AI and data analytics, operationalised through the Veritas methodology. — [MAS FEAT](https://www.mas.gov.sg/publications/monographs-or-information-paper/2018/feat) [KB]
- **MAS Information Paper on AI Model Risk Management** (Dec 2024, circular ID 18/24): based on a mid-2024 thematic review of banks' AI and GenAI MRM. Good practices cover:
  - AI governance forums and policies;
  - AI inventories and materiality assessment;
  - data management and validation;
  - GenAI hallucination and evaluation challenges;
  - for **third-party AI**: testing, contingency plans, legal updates and staff training.
  - Sources: [MAS information paper](https://www.mas.gov.sg/publications/monographs-or-information-paper/2024/artificial-intelligence-model-risk-management); [PDF](https://www.mas.gov.sg/-/media/mas-media-library/publications/monographs-or-information-paper/imd/2024/information-paper-on-ai-risk-management-final.pdf); [K&L Gates](https://www.klgates.com/Managing-Artificial-Intelligence-The-Monetary-Authority-of-Singapores-Recommendations-on-AI-Model-Risk-Management-1-22-2025)
- **Proposed MAS Guidelines on AI Risk Management** (consultation 13 Nov 2025, closed 31 Jan 2026), **proposed, not final** as of Sept 2026:
  - They apply to all FIs, "including Generative AI and AI agents".
  - Expectations: board and senior management oversight; "accurate and up-to-date AI inventories" and risk materiality assessments; lifecycle controls covering data management, fairness, transparency, human oversight, **third-party risk management**, testing, monitoring and change management.
  - They are proportionate, with a 12-month transition after issuance.
  - Sources: [MAS media release](https://www.mas.gov.sg/news/media-releases/2025/mas-guidelines-for-artificial-intelligence-risk-management); [MAS consultation paper](https://www.mas.gov.sg/publications/consultations/2025/consultation-paper-on-guidelines-on-artificial-intelligence-risk-management); [Allen & Gledhill](https://www.allenandgledhill.com/sg/publication/articles/31741/mas-consults-on-proposed-guidelines-for-artificial-intelligence-risk-management); [ASIFMA response](https://www.asifma.org/wp-content/uploads/2026/02/2026-01-31-asifma-response-to-mas-ai-risk-mgmt-guidelines-cp.pdf)
- **Project MindForge**:
  - An MAS-led industry consortium; the toolkit was built with 24 banks, insurers, capital-market firms and other partners.
  - AI Risk Management **Executive Handbook** (Nov 2025) and **Operationalisation Handbook** (Jan 2026), covering traditional, generative and agentic AI, plus case studies.
  - Sources: [Project MindForge page](https://www.mas.gov.sg/schemes-and-initiatives/project-mindforge); [Operationalisation Handbook PDF](https://www.mas.gov.sg/-/media/mas-media-library/schemes-and-initiatives/ftig/project-mindforge/mindforge-ai-risk-management-operationalisation-handbook.pdf); [MAS 2026 toolkit release](https://www.mas.gov.sg/news/media-releases/2026/mas-partners-industry-to-develop-ai-risk-management-toolkit-for-the-financial-sector)
- **PDPC Advisory Guidelines** on use of personal data in AI recommendation and decision systems (Mar 2024) point to data minimisation, pseudonymisation and anonymisation during development. — [PDPC advisory guidelines](https://www.pdpc.gov.sg/guidelines-and-consultation/2024/02/advisory-guidelines-on-use-of-personal-data-in-ai-recommendation-and-decision-systems) [KB]

### Inferences
- Singapore's banking secrecy makes customer information in prompts a hard, statutory problem. Only data that is not "customer information", such as properly pseudonymised or redacted data, escapes s.47. That favours local redaction more strongly than jurisdictions that rely on contracts alone.
- Once the AI Risk Management Guidelines are finalised, coding agents will need to be in the AI inventory, rated for materiality, and managed as third-party AI. That creates a documentation and audit-trail workload that tooling can automate.

### Gaps
- The exact legal status and date of MAS Notice 658 and the revised MAS Guidelines on Outsourcing (banks), reportedly Dec 2024, were not verified this session.
- The MAS information paper on cyber risks of GenAI (reportedly 2024) was not verified.
- The finalisation date of the AI Risk Management Guidelines is unknown. As of Sept 2026, MAS was still reviewing feedback.

---

## Q6. Supervisory statements or enforcement actions specifically about GenAI data leakage in financial services

### Takeaway
I found **no public enforcement action by a financial regulator (US, EU, UK or Singapore) specifically penalising a firm for leaking customer data or source code through prompts to a GenAI tool** as of Sept 2026. Supervisory attention is instead expressed through three things:
- guidance: the NYDFS 2024 letter, the FINRA 2026 report, the PCI SSC 2025 AI Principles and the MAS 2024 paper;
- surveys: BoE/FCA;
- one data-protection enforcement against a model provider (the Italian Garante against OpenAI), which is not FS-specific.

### Cited Findings
- NYDFS names NPI exposure through AI products and supply-chain risk. It expects monitoring for "unusual query behaviors that might indicate an attempt to extract NPI." — [NYDFS letter](https://www.dfs.ny.gov/industry-guidance/industry-letters/il20241016-cyber-risks-ai-and-strategies-combat-related-risks)
- FINRA's 2026 report warns that AI agents "may unintentionally store, explore, disclose, or misuse sensitive or proprietary information". — [Debevoise](https://www.debevoisedatablog.com/2025/12/11/finras-2026-regulatory-oversight-report-continued-focus-on-generative-ai-and-emerging-agent-based-risks/); [FINRA report](https://www.finra.org/sites/default/files/2025-12/2026-annual-regulatory-oversight-report.pdf)
- PCI SSC says AI "should not be trusted with high-impact secrets or unprotected sensitive data". — [PCI SSC AI Principles](https://blog.pcisecuritystandards.org/ai-principles-securing-the-use-of-ai-in-payment-environments)
- BoE/FCA: data privacy is the top perceived AI risk and data protection the largest perceived regulatory constraint. — [BoE/FCA 2024 survey](https://www.bankofengland.co.uk/report/2024/artificial-intelligence-in-uk-financial-services-2024)
- Italian Garante fined OpenAI €15m (Dec 2024) over ChatGPT GDPR failures, following a March 2023 incident that exposed chat histories and partial payment details. A secondary source reports that a Rome court **annulled** the fine in March 2026 (reasoning unpublished). — [Syrenis (secondary)](https://syrenis.com/resources/chatgpt-gdpr/). *Unverified with a primary source; treat with caution.*
- UK regulators acknowledged in 2026 that GenAI and agentic adoption does not yet pose systemic risk, but that risk "likely to increase, potentially rapidly". — [BoE response to TSC](https://www.bankofengland.co.uk/-/media/boe/files/letter/2026/response-to-tsc-inquiry-report-on-ai-in-financial-services)

### Inferences
- There is no enforcement precedent, so banks' controls are driven by *ex ante* supervisory expectations and general rules, not case law. The practical trigger for enforcement would be an ordinary breach-notification event under GDPR Art. 33, Reg S-P, the FTC Safeguards Rule, NYDFS §500.17 or MAS incident notification, where the leaked data happened to pass through an LLM vendor.

### Gaps
- Widely reported 2023 restrictions on employee ChatGPT use at large banks were not re-verified with primary sources in this session.
- No regulator-published incident statistics on GenAI data leakage in FS were found.

---

## Q7. Which obligations favour technical guarantees over contractual ones, and what a privacy-preserving coding agent could help satisfy

### Takeaway
Most FS third-party rules (US TPRM, SS2/21, DORA Art. 30, EBA outsourcing, GDPR Art. 28) can be met **contractually**: DPAs, audit rights, data-location clauses, breach notice and vendor ZDR/enterprise terms. NYDFS even recommends contracting for vendors' enhanced privacy options. Several obligations, though, can in practice **only** be met, or are met far more cheaply, by technical guarantees:
- GDPR transfer supplementary measures where the vendor needs cleartext (EDPB Use Case 6);
- pseudonymisation that is strong against the recipient (SRB);
- PCI scope reduction (tokenisation before egress);
- Singapore banking secrecy;
- DORA/ECB data-leakage prevention and encryption "in use";
- logging and monitoring duties (NYDFS §500.6/.14, the FTC Safeguards logging requirement, AI Act Art. 12/26 logs, PCI Req. 10 and AI Principles prompt logging);
- exit and concentration-risk duties (DORA Art. 28, ECB exit plans).

A vendor ZDR promise is a contractual assurance the bank cannot independently verify. It supports due diligence, but it does not satisfy "monitoring" or "logging" expectations, and it does not reduce PCI or banking-secrecy scope.

### Cited Findings

**Obligations that favour technical guarantees:**
- **EDPB Recommendations 01/2020**:
  - For transfers where the importer needs data in the clear under problematic public-authority access laws, the EDPB found no effective technical measure (Use Cases 6–7).
  - Pseudonymisation with the key kept in the EEA (Use Case 2) can be an effective supplementary measure.
  - Contractual measures alone are generally insufficient where the laws of the destination country undermine them.
  - Source: [EDPB Recs 01/2020](https://www.edpb.europa.eu/our-work-tools/our-documents/recommendations/recommendations-012020-measures-supplement-transfer_en) [KB]
  - The DPF currently removes this for certified US vendors, but the DPF's durability is litigated. — [CURIA T-553/23](https://curia.europa.eu/juris/liste.jsf?num=T-553/23) [KB]
- **CJEU SRB**: data a recipient cannot re-identify may not be personal data for that recipient. That benefit exists only if the controller implements the pseudonymisation technically and keeps the key. — [CURIA press release](https://curia.europa.eu/site/upload/docs/application/pdf/2025-09/cp250107en.pdf)
- **GDPR Arts. 25 and 32** name pseudonymisation and encryption as measures. — [GDPR](https://eur-lex.europa.eu/eli/reg/2016/679/oj) [KB]
- **ECB cloud guide**: encryption "in transit, at rest and, where feasible, in use", plus costed exit plans before go-live. — [Jones Day on ECB guide](https://www.jonesday.com/en/insights/2025/11/new-ecb-guide-on-outsourcing-cloud-services-to-cloud-service-providers)
- **DORA Art. 9** (tools preventing confidentiality breaches and data loss) and **Art. 28** (exit strategies and concentration risk). — [DORA](https://eur-lex.europa.eu/eli/reg/2022/2554/oj) [KB]
- **PCI SSC**: tokens or single-use PANs, sanitisation before use, logging of prompt inputs and AI actions. — [PCI SSC AI Principles](https://blog.pcisecuritystandards.org/ai-principles-securing-the-use-of-ai-in-payment-environments)
- **NYDFS**: monitoring for NPI-extraction query patterns; AI systems in the asset inventory (Nov 2025); NPI disposal. — [NYDFS letter](https://www.dfs.ny.gov/industry-guidance/industry-letters/il20241016-cyber-risks-ai-and-strategies-combat-related-risks)
- **FTC Safeguards Rule**: log and monitor authorized users' activity; data inventory; disposal. — [FTC Safeguards Rule](https://www.ftc.gov/legal-library/browse/rules/safeguards-rule) [KB]
- **AI Act** Art. 12 (automatic logging) and Art. 26 (deployers keep logs for at least 6 months) for high-risk systems, from 2 Dec 2027 for Annex III. — [AI Act](https://eur-lex.europa.eu/eli/reg/2024/1689/oj) [KB]; [CSA note on Omnibus dates](https://labs.cloudsecurityalliance.org/research/csa-research-note-eu-ai-act-high-risk-deadline-omnibus-20260/)
- **FINRA 2026**: track agent actions and restrict system access. — [Debevoise](https://www.debevoise.com/insights/publications/2025/12/finras-2026-regulatory-oversight-report-continued)
- **MAS** (proposed): AI inventories, materiality assessment, lifecycle controls including data management and third-party risk. — [MAS media release](https://www.mas.gov.sg/news/media-releases/2025/mas-guidelines-for-artificial-intelligence-risk-management)
- **Treasury 2024**: industry points to PETs (homomorphic encryption, federated learning) as the remedy for data risk. — [Debevoise summary](https://www.debevoisedatablog.com/2025/01/23/treasurys-post-2024-rfi-report-on-ai-in-financial-services-uses-opportunities-and-risks/)

**Obligations met mainly by contract or vendor assurance:**
- **US interagency TPRM (2023)**: contract terms on confidentiality, data use, audit and breach notice, plus ongoing monitoring. — [Fed press release](https://www.federalreserve.gov/newsevents/pressreleases/bcreg20230606a.htm) [KB]
- **GLBA Interagency Guidelines**: contracts requiring service providers to implement safeguards. — [eCFR](https://www.ecfr.gov/current/title-12/chapter-III/subchapter-B/part-364/appendix-Appendix%20B%20to%20Part%20364) [KB]
- **DORA Art. 30** mandatory clauses and **SS2/21** audit and access rights (contractual, but they must be exercisable). — [DORA](https://eur-lex.europa.eu/eli/reg/2022/2554/oj) [KB]; [PRA SS2/21](https://www.bankofengland.co.uk/prudential-regulation/publication/2021/march/outsourcing-and-third-party-risk-management-ss) [KB]
- **Reg S-P**: service-provider oversight and 72-hour breach notification by the provider. — [Holland & Knight](https://www.hklaw.com/en/insights/publications/2026/05/regulation-s-p-amendments-compliance-deadline-approaching)
- **NYDFS**: "representations and warranties" and requiring vendors to "take advantage of available enhanced privacy, security, and confidentiality options". — [NYDFS letter](https://www.dfs.ny.gov/industry-guidance/industry-letters/il20241016-cyber-risks-ai-and-strategies-combat-related-risks)
- **CTP/CTPP regimes**: regulator oversight of hyperscalers (AWS, Google, Microsoft, Oracle in the UK; 19 CTPPs in the EU). This gives indirect assurance for LLMs hosted on those clouds, but not for direct frontier-model APIs. — [BoE](https://www.bankofengland.co.uk/news/2026/july/uk-financial-regulators-to-begin-overseeing-critical-third-parties-announced-by-hmt); [EBA](https://www.eba.europa.eu/publications-and-media/press-releases/european-supervisory-authorities-designate-critical-ict-third-party-providers-under-digital)

### Inferences
Concrete obligations a privacy-preserving coding agent (local or redacting, with verifiable logs) could help satisfy, each mapped to its source:

1. **Data minimisation and pseudonymisation before egress.**
   - Sources: GDPR 5(1)(c)/25/32; EDPB 01/2025; SRB; PCI AI Principles tokenisation; NYDFS NPI minimisation; Singapore s.47; PDPC.
   - The agent could detect and replace PII, NPI, PAN, secrets and customer identifiers in code, fixtures, logs and diffs, keep the mapping key local, and restore it on the way back.
2. **A verifiable audit trail.**
   - Sources: NYDFS §500.6/§500.14; FTC Safeguards logging; PCI Req. 10 and AI Principles prompt logging; AI Act Art. 12/26 logs; FINRA "track agent actions"; MAS inventories and monitoring.
   - The agent could keep a tamper-evident record of every outbound prompt, what was redacted, which model and endpoint were used, and which human approved each action.
3. **Third-party risk evidence.**
   - Sources: DORA register of information and Art. 28/30; US TPRM; SS2/21; Reg S-P; MAS third-party AI.
   - The agent could provide machine-readable records of which vendor received which data classes. That data feeds the DORA register and TPRM monitoring, and shows the vendor never received NPI or PAN, shrinking the scope of vendor due diligence.
4. **Operational resilience and exit.**
   - Sources: DORA Art. 28(8) exit strategies; ECB costed exit plans; UK impact tolerances; concentration concerns (BoE/FCA survey: top-3 model providers hold 44%).
   - A backend-agnostic agent that can fail over to local or on-prem models shows substitutability and a workable exit.
5. **AI inventory and model-risk documentation.**
   - Sources: NYDFS asset inventory including AI; MAS proposed AIRG; SS1/23; AI Act literacy.
   - US federal MRM now excludes GenAI, but UK SS1/23 and MAS still expect inventories and third-party model oversight.
6. **Human-in-the-loop and least-privilege credentials.**
   - Sources: PCI AI Principles; FINRA agent guidance; MAS human oversight.

Overall: contractual ZDR plus an enterprise DPA is usually enough for **non-sensitive** code in the US and UK. Technical "data never leaves" or "leaves only pseudonymised" guarantees become **decisive** for:
- PAN, SAD or secrets (PCI);
- Singapore customer information (banking secrecy);
- EU personal data going to non-DPF or cleartext-requiring third-country processors (EDPB Use Case 6);
- any case where the bank must *demonstrate* rather than *assert* what left its perimeter (monitoring and logging duties).

### Gaps
- No regulator has explicitly said that vendor zero-data-retention terms are sufficient or insufficient for FS use. This comparison is an inference from the general rules above.
- Whether any supervisor has accepted the SRB relative-pseudonymisation approach specifically for LLM API use is untested.
- Whether confidential computing or TEE-based LLM inference counts as "encryption in use" under the ECB guide has not been addressed publicly in any source found.
