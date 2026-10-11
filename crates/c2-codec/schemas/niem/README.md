# NIEM 6.0 exchange for c2-fusion

The `niem-xml` codec (`crates/c2-codec/src/niem_xml.rs`) reads and writes the four c2-fusion message types as NIEM 6.0 XML messages. This directory holds the message specification: the schemas the messages conform to, sample instances, and this conformance statement.

## Message types

Each message type is identified by its root element, the initial property in NDR 6.0 terms.

| Canonical message | Root element | Built from |
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

NIEM components are reused wherever their meaning matches. The extension namespace adds only what NIEM 6.0 does not have: the four message types, `c2f:OperationalDomainCode` (LAND, AIR, MARITIME, SPACE, CYBER), `c2f:FunctionalServiceCode` (COMMAND_AND_CONTROL, INTELLIGENCE, LOGISTICS, MEDICAL) and `c2f:RequestingOrganization`, an `nc:OrganizationType` element, because `nc:RequestType` has no requester. `usmtf:AvailablePersonnelQuantity` is documented for 0 to 9999 personnel; the codec refuses to encode a larger situation report rather than misstate it.

The other files under `6.0/` are the unmodified NIEM 6.0 schemas that `niem-core.xsd` and `mo.xsd` import, kept at their original relative paths.

## Validation

Every encoded message is validated against these schemas in the codec's tests, and so are the samples in `samples/`:

```
cargo test -p c2-codec niem_xml
```

The tests run `xmllint --noout --schema extension/c2-fusion.xsd`, so libxml2's `xmllint` must be installed where tests run (it ships with macOS; on Debian and Ubuntu it is `libxml2-utils`, which CI installs). A sample can also be checked by hand from this directory:

```
xmllint --noout --schema extension/c2-fusion.xsd samples/contact.xml
```

## Scope and limits

- XML only. NIEM 6.0 also defines a JSON-LD representation (NDR 6.0 section 13); it is not implemented yet.
- The four schema sets are not packaged as a formal message specification catalog, because no NIEM 6.0 packaging specification was found to package them against; this README is the conformance statement.
- Validation covers XML Schema. The NDR's Schematron rules for schema documents were not run.

## Licence and attribution

The files under `6.0/` are from NIEM Model Version 6.0 by OASIS Open (https://docs.oasis-open.org/niemopen/niem-model/v6.0/os/), licensed under Creative Commons Attribution 4.0 International (https://creativecommons.org/licenses/by/4.0/). `6.0/domains/mo-usmtf.xsd` is a subset of the original usmtf domain schema. The extension schema and samples are part of c2-fusion and share its licence.
