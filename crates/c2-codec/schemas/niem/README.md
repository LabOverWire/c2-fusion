# NIEM 6.0 exchange for c2-fusion

Two codecs read and write the four c2-fusion message types as NIEM 6.0 messages: `niem-xml` (`crates/c2-codec/src/niem_xml.rs`) in the XML representation and `niem-json` (`crates/c2-codec/src/niem_json.rs`) in the JSON-LD representation defined by NDR 6.0 section 13. This directory holds the message specification: the schemas the messages conform to, sample instances in both representations, and this conformance statement.

## Message types

Each message type is identified by its root element (XML) or its single message property (JSON), the initial property in NDR 6.0 terms.

| Canonical message | Message property | Built from |
|---|---|---|
| Contact report | `c2f:ContactReportMessage` | `nc:ReportType` with `mo:Observation` (observation time, observed location as `nc:Location2DGeospatialCoordinate` latitude and longitude) |
| Situation report | `c2f:SituationReportMessage` | `nc:ReportType` with `nc:DocumentSummaryText` and `usmtf:AvailablePersonnelQuantity` |
| Request for information | `c2f:InformationRequestMessage` | `nc:RequestType` with `nc:RequestText` and the requesting organization |
| Own-unit position report | `c2f:PositionReportMessage` | `nc:ReportType` with `mo:Unit` (`mo:UnitName`, `mo:UnitPresentLocation`) |

All four carry the reporting or requesting unit, the message identifier and time in NIEM Core elements (`nc:DocumentAuthor`, `nc:DocumentIdentification`, `nc:DocumentCreationDate`, `nc:RequestIdentification`, `nc:RequestDate`), plus an operational domain and a functional service code.

## Namespaces and conformance targets

| Namespace | Prefix | Schema | Conformance target |
|---|---|---|---|
| `https://docs.oasis-open.org/niemopen/ns/model/niem-core/6.0/` | `nc` | `6.0/niem-core.xsd` | Reference schema document (unmodified) |
| `https://docs.oasis-open.org/niemopen/ns/model/domains/militaryOperations/6.0/` | `mo` | `6.0/domains/mo.xsd` | Reference schema document (unmodified) |
| `https://docs.oasis-open.org/niemopen/ns/model/domains/usmtf/6.0/` | `usmtf` | `6.0/domains/mo-usmtf.xsd` | Subset schema document: only `AvailablePersonnelQuantity` and its two types, copied unchanged from the reference schema |
| `https://laboverwire.com/ns/c2-fusion/1.0/` | `c2f` | `extension/c2-fusion.xsd` | Extension schema document |

NIEM components are reused wherever their meaning matches. The extension namespace adds only what NIEM 6.0 does not have: the four message types, `c2f:OperationalDomainCode` (LAND, AIR, MARITIME, SPACE, CYBER), `c2f:FunctionalServiceCode` (COMMAND_AND_CONTROL, INTELLIGENCE, LOGISTICS, MEDICAL) and `c2f:RequestingOrganization`, an `nc:OrganizationType` element, because `nc:RequestType` has no requester.

The other files under `6.0/` are the unmodified NIEM 6.0 schemas that `niem-core.xsd` and `mo.xsd` import, kept at their original relative paths. NIEMOpen's `cmftool` recognises all four namespaces above as conforming NIEM 6.0 namespaces, and its model validator (`mval`) accepts the model built from this schema set.

## Values the codecs refuse

Both codecs check a message against the schema facets before encoding and refuse it rather than emit an invalid message:

- `mo:UnitName` (position reports) must be a token of 4 to 100 characters;
- latitude must be within -90 to 90 and longitude within -180 to 180 degrees;
- times must be `xs:dateTime` values, and `mo:ObservationDateTime` must be in UTC (`Z`);
- `usmtf:AvailablePersonnelQuantity` is documented for 0 to 9999 personnel, so a larger situation report is refused rather than misstated.

## JSON representation

A `niem-json` message is a JSON object holding an embedded `@context`, which maps the four prefixes to their namespace URIs (NDR 6.0 rule 13-4), and exactly one message property. When decoding, keys are expanded through the sender's `@context`, so a message that uses other prefixes for the same namespaces decodes the same way.

`json/c2-fusion-messages.schema.json` is the JSON message schema for the four message properties. It was generated from this schema set by NIEMOpen's `cmftool` 1.1.0 (https://github.com/niemopen/cmftool), which needs Java 25, run from a copy of this directory at a path without spaces:

```
cmftool x2m -o c2f.cmf extension/c2-fusion.xsd
cmftool m2jmsg -m c2f:ContactReportMessage,c2f:SituationReportMessage,c2f:InformationRequestMessage,c2f:PositionReportMessage -o json/c2-fusion-messages.schema.json c2f.cmf
```

The generator warns that JSON Schema cannot enforce three XSD facets: `totalDigits` on `mo:ForceManagementIDType`, and `maxOccurs="1"` on `mo:SecurityClassificationAbstract` and `mo:ResourceIDAbstract`. None of them are in the elements these messages use. Regenerating with the commands above reproduces the committed file byte for byte.

## Validation

Every encoded message, and every sample in `samples/`, is validated in the codecs' tests: XML against the schema set with libxml2's `xmllint`, JSON against the generated JSON schema with the `jsonschema` crate. Tests also confirm that each validator rejects an out-of-range latitude.

```
cargo test -p c2-codec niem
```

`xmllint` must be installed where tests run (it ships with macOS; on Debian and Ubuntu it is `libxml2-utils`, which CI installs). A sample can also be checked by hand from this directory:

```
xmllint --noout --schema extension/c2-fusion.xsd samples/contact.xml
```

## Scope and limits

- The schemas are not packaged as a formal message specification catalog, because no NIEM 6.0 packaging specification was found to package them against; this README is the conformance statement.
- Validation covers XML Schema and the generated JSON Schema. The NDR's Schematron rules for schema documents were not run.
- The JSON `@context` is embedded in each message; remote contexts are not supported when decoding.

## Licence and attribution

The files under `6.0/` are from NIEM Model Version 6.0 by OASIS Open (https://docs.oasis-open.org/niemopen/niem-model/v6.0/os/), licensed under Creative Commons Attribution 4.0 International (https://creativecommons.org/licenses/by/4.0/). `6.0/domains/mo-usmtf.xsd` is a subset of the original usmtf domain schema, and `json/c2-fusion-messages.schema.json` is generated from these schemas. The extension schema and samples are part of c2-fusion and share its licence.
