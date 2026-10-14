# coie-text: character-oriented message format

The `coie-text` codec (`crates/c2-codec/src/coie_text.rs`) reads and writes the four c2-fusion message types as slash-delimited, character-oriented text in the style of NATO ADatP-3 / APP-11 and US MTF messages. The IDEaS challenge calls this Character-Oriented Information Exchange (COIE).

It is a format built on the publicly documented structure of those messages. It is **not** an APP-11 conformant implementation: the APP-11 message catalogue and the ADatP-3 rules are distributed through the NATO Standardization Office and national authorities only, and the per-message set definitions below are c2-fusion's own until they can be checked against APP-11(E).

## Example

```
MSGID/SPOTREP/UAV-7/C-1/SEP//
TIME/2026-09-29T14:30:00Z//
POSITION/45.5017/-73.5673//
CLASS/AIR/INTELLIGENCE//
AMPN/Vehicle stationary, N/S road//
```

## Grammar

- A message is a sequence of sets. A set is a set identifier followed by one or more fields, each introduced by `/`, and the set ends with `//`.
- Whitespace and line breaks between sets are ignored. The encoder writes one set per line.
- A field holding `-` has no data. The encoder writes an empty value as `-`, so a value that is literally `-` cannot be encoded and is refused.
- In ordinary sets a field cannot contain `/` or a line break.
- `AMPN` and `NARR` are free-text sets: their single field runs to the closing `//`, so it may contain single `/` characters but not `//`, cannot start or end with `/`, and cannot contain a line break.
- The message must contain exactly the sets listed for its identifier, in that order, and nothing else.
- A message carrying a confidentiality label is refused: the format has no set for it yet.
- A decoded message must pass the same checks as one being encoded: coordinates are finite and within range, `TIME` is a valid `xs:dateTime` (a real calendar date), and the id is 1 to 255 bytes. A message that fails them is refused as invalid.

## Messages

Every message starts with `MSGID/identifier/originator/serial/month//`: the originator is the reporting or requesting unit, the serial is the message id, and the month is the three-letter month of the message time, which must agree with `TIME`.

| Canonical message | Identifier | Sets after MSGID |
|---|---|---|
| Contact report | `SPOTREP` | `TIME/observed-at//`, `POSITION/latitude/longitude//`, `CLASS/domain/service//`, `AMPN/description//` |
| Situation report | `SITREP` | `TIME/reported-at//`, `PERS/personnel-effective//`, `CLASS/domain/service//`, `NARR/summary//` |
| Request for information | `RFI` | `TIME/requested-at//`, `CLASS/domain/service//`, `NARR/question//` |
| Own-unit position report | `POSREP` | `TIME/reported-at//`, `POSITION/latitude/longitude//`, `CLASS/domain/service//` |

Times are ISO 8601 UTC date-times as held by the canonical model. Latitude and longitude are decimal degrees. Domains are `LAND`, `AIR`, `MARITIME`, `SPACE`, `CYBER`; services are `COMMAND_AND_CONTROL`, `INTELLIGENCE`, `LOGISTICS`, `MEDICAL`.

## What comes from public sources and what is provisional

From public sources:

- the set, field and segment structure, the `/` field separator and `//` set terminator, and `-` as the no-data sign (US Army FM 34-43, https://www.globalsecurity.org/intell/library/policy/army/fm/34-43/ch3.htm; METU thesis on ADatP-3, https://etd.lib.metu.edu.tr/upload/1224682/index);
- the `MSGID` field order of identifier, originator, serial number and month (FM 34-43; a released example is `MSGID/GENADMIN/CNO WASHINGTON DC/N3N5/JUN//` in NAVADMIN 182/11);
- `SITREP` and `RFI` as message names in use (FM 101-5-2, https://www.bits.de/NRANEU/others/amd-us-archive/fm101-5-2(uk).pdf).

Provisional, c2-fusion's own until checked against APP-11(E):

- the identifiers `SPOTREP` and `POSREP` and the APP-11 identifiers for all four messages;
- the `TIME`, `POSITION`, `CLASS` and `PERS` sets and their field formats, and `AMPN` / `NARR` as the free-text sets; real ADatP-3 messages use date-time groups and position formats that do not keep the canonical model's seconds;
- the character set: this codec accepts any UTF-8 text, while ADatP-3 restricts the character repertoire.

## Conversion to XML

Because both codecs map to the same canonical model, a `coie-text` message converts losslessly to the NIEM 6.0 XML form (`niem-xml`) and back; the test `character_oriented_text_converts_to_niem_xml_and_back` checks this.
