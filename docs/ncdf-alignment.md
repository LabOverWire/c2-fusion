# NCDF alignment (design level)

This note describes how c2-fusion lines up with the NATO Core Data Framework (NCDF) and what an NCDF codec would take. It is a design-level note, not a conformance claim: c2-fusion produces no NCDF artefact today, and the NCDF specifications are not publicly available.

## Status of NCDF

- The framework is ADatP-5653 Edition A Version 1 (STANAG 5653), and its API is ADatP-5659 Edition A Version 1. NATO's C3 architecture repository lists both with status "Study", published by the NATO Standardization Office, and their texts sit behind the NSO's protected portal ([ADatP-5653](https://nhqc3s.hq.nato.int/apps/DCRA_Report/id-29d4122b072148f5aaf4882ecc5d963c/elements/id-9953775519144b2189df7349eabbadd4.html), [ADatP-5659](https://nhqc3s.hq.nato.int/apps/DCRA_Report/id-29d4122b072148f5aaf4882ecc5d963c/elements/id-f2564695b21a4d51a1c0112d3bfb491e.html)).
- The NISP lists ADatP-5653 under the Cross Community Information Sharing service profile, with the DPC CaP1 Data Management Capability Team responsible ([NISP entry](https://nisp.nw3.dk/standard/nato-adatp5653-ed.a-v1.html)).
- Everything below about NCDF's structure comes from public briefings, mainly the NCDF CWIX 2022 briefing to the NIEM MilOps stakeholder meeting of 10 August 2022 ([slides](http://niem.github.io/community/milops/meeting-notes/NIEM_MilOps_10Aug22_StakeholderEngagement.pdf)).

## What is publicly known

The CWIX 2022 briefing presents NCDF under the heading "NIEM for NATO" and describes it as "a framework of standards, specifications, tools, policies" whose purpose is "to improve data and information interoperability in the Alliance and with mission partners". It names four components:

1. an agreed Semantic Reference Model (SRM);
2. XML Naming and Design Rules (XNDR);
3. a Standards Transformation Framework (STF);
4. Information Exchange Patterns (IEPs).

The same slide shows a metadata model with `ConceptMetadata`, `BattlespaceConceptMetadata` and a `NilReason` structure (a lacking code value or another nil reason), and links information exchange requirements to information exchange specifications.

The briefing also describes the NCDF Data Lake tested at CWIX 2022: national and NATO systems contribute ADatP-3 MTF messages (for example NAVSITREP and CBRN messages), a transformation service (INT-CORE) translates them into NCDF for the lake, and consumers retrieve the data translated back into their own formats. It reports four data lake nodes synchronised (NCIA, two German, one Spanish), a validated Data Lake API, and as improvement areas "Fully define Data Lake input/output to ensure 100% data transformation" and more rigorous schema validation before ingestion. Its 2023 plan lists updated Naming and Design Rules and STANAG ratification.

Data-centric security work around NCDF uses confidentiality labels: an ASMG briefing shows an information exchange framework that provisions data to NCDF and uses policy to "add 4774 labels to selected messages", redact selected elements and route information to the right networks ([ASMG, Introduction to Data Centric Security](https://cdn.ymaws.com/www.globalaea.org/resource/collection/8B88871F-6CF5-47C4-A827-D0ABA8B4F469/13_June_-3_-_Abramson_-_Introduction_to_Data_Centric_Security__ASMG_.pdf)), and an OMG white paper models an NCDF semantic element `NCDF-BSO-Vessel` built from five data patterns, among them the vessel's identity, type and position ([OMG/ASMG white paper, 2021](https://www.omg.org/news/whitepapers/20210125-ASMG_DCS_S2S_Policy-Modelling-v1-02.pdf)). STANAG 4774 defines confidentiality metadata labels and STANAG 4778 binds them to data.

## How c2-fusion lines up

| NCDF component | c2-fusion counterpart | Alignment today |
|---|---|---|
| Semantic Reference Model | The canonical model (`crates/c2-model`): contact report, situation report, request for information, own-unit position report, each with unit, domain, functional service, time and, where relevant, location | Every codec maps to and from this one model, which is the role an SRM plays. The SRM itself is not public, so the concepts are mapped to NIEM 6.0 Core and the Military Operations domain instead (`crates/c2-codec/schemas/niem/README.md`), the closest open analogue given NCDF's "NIEM for NATO" lineage. |
| XML Naming and Design Rules | The `niem-xml` and `niem-json` codecs follow the NIEM 6.0 NDR: reference schemas reused unmodified, a subset schema, an extension schema in its own namespace for what NIEM lacks, and validation of every encoded message against the schema set | The NCDF XNDR is not public. If it follows the NIEM NDR, as the briefing's framing suggests, the same pattern (reuse, subset, extension namespace) carries over. |
| Standards Transformation Framework | The codec registry (`crates/c2-codec`): a format attaches as a `Codec` with `encode` and `decode` against the canonical model, so any registered format translates to any other through the model | This is the same hub-and-spoke shape as the INT-CORE service in the Data Lake, which translates ADatP-3 MTF into NCDF and back. An NCDF codec would be one more registered codec, and messages from the `coie-text` and `mtf-xml` codecs would translate to it through the model. |
| Information Exchange Patterns | Two levels: message specifications (the NIEM schema set, samples and conformance statement) and exchange patterns (replicated writes with per-origin sequence numbers, cursor-based catch-up after loss or partition, presence leases) | The message specification side follows the NIEM message specification layout. The exchange side is a synchronised, multi-node store that converges after partition, the same job the Data Lake's node synchronisation does, but built for nodes on degraded and intermittent links. |
| Metadata (`ConceptMetadata`, nil reasons, STANAG 4774/4778 labels) | None yet | The canonical model carries no security label, source or nil-reason metadata. This is the main design gap. |

## Design for an NCDF codec

When the specification becomes available, the work is:

1. **Metadata envelope.** Add an envelope to the canonical model carrying a STANAG 4774 confidentiality label (a policy, a classification and categories), the originating system, and per-field nil reasons. Because the envelope is part of the record, replication carries it with every write and its origin stamp over every bearer; the compact LoRa encoding would need to make room for it. Every codec maps the envelope: NIEM to its metadata structures, NCDF to `ConceptMetadata`, and formats without metadata refuse or drop it explicitly rather than silently.
2. **Semantic mapping table.** For each canonical message, map each field to an SRM concept, recording where the SRM has no match and the concept goes into an extension namespace, as the NIEM mapping does today.
3. **Codec.** Implement `ncdf-xml` (and a JSON form if the XNDR defines one) on the existing `Codec` interface, validating each encoded message against the NCDF schemas in tests, the way the NIEM codecs validate with `xmllint` and the generated JSON schema.
4. **API adapter.** Expose the shared picture through the ADatP-5659 API so a data lake or other NCDF consumer can read from and write to a c2-fusion node. This is an adapter in front of the store, separate from the codec.

Steps 2 to 4 need the NCDF specifications. Step 1 does not, and can be built and tested now against the STANAG 4774 label structure.

## Scope and limits

- No NCDF schema, SRM, XNDR or API definition was available for this note; the alignment is by design and by analogy with NIEM 6.0, and every statement about NCDF's structure is sourced from the public briefings cited above.
- The published Q&A for this IDEaS challenge (CFP 006, W7714-248676/017) answers the NCDF question (Question 28; the answer is printed as Answer 29 because the answers in that range are shifted) with "Canada does not require implementation of a specific NCDF specification or profile", evaluating instead the capability for structured, interoperable, standards-based exchange "rather than conformance to a specific NCDF implementation artefact". Answer 13 states that Canada has not committed to furnish NCDF documentation for Component 1a.
- The relationship between NCDF and NIEM is taken from the CWIX briefing's "NIEM in NATO" and "NIEM for NATO" headings; it is not confirmed against ADatP-5653.
