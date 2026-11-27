# EXIG: EXI grammars as EXI

Status: draft 1, unversioned. Nothing outside this repository has used it, and it can
still change. The namespace `urn:exig:draft-1` is a placeholder until one is assigned.

An `.exig` file holds the schema-informed grammars EXI derives from a schema (EXI 1.0
§8.5). A receiver needs only an EXI decoder and the EXIG schema to read it, and no XSD
compiler. Equal grammars give byte-equal files, so the file's SHA-256 names the grammars.

The schema is [`schemas/exig-1.xsd`](schemas/exig-1.xsd). The implementation is
[`src/grammar/exig.rs`](src/grammar/exig.rs).

## The file

An `.exig` file is an EXIG document encoded as EXI 1.0:

- schema-informed under `schemas/exig-1.xsd`;
- strict;
- byte-aligned;
- the options in the header, and no cookie;
- no schemaId and no preserve options.

A reader rejects a file whose header has no options or is not strict.

The digest is `sha256:` followed by the 64 lowercase hex digits of the SHA-256 of the
file. It can be used as an EXI `schemaId`. A file does not carry a name of its own: its
name is this digest, which the file cannot contain.

The `exi` tool works with these files:

```sh
exi compile schema.xsd -o schema.exig   # XSD to .exig
exi exig schema.exig                    # .exig as XML text
exi digest schema.exig                  # the sha256: name
```

## Build a fixed grammar into firmware

A firmware crate can put `.exig` support on its host build dependency and leave it
off its target dependency. Use Cargo resolver 2 or later. Edition 2024 uses resolver
3 by default for a standalone crate.

```toml
[dependencies]
exi = { path = "../exi", default-features = false, features = ["bit-packed"] }

[build-dependencies]
exi = { path = "../exi", default-features = false, features = ["std", "exig"] }
```

Omit `bit-packed` when the firmware uses only byte-aligned bodies. In the firmware
crate's `build.rs`:

```rust
fn main() {
    let input = "sensor.exig";
    println!("cargo:rerun-if-changed={input}");
    let bytes = std::fs::read(input).expect("read sensor.exig");
    let source = exi::grammar::exig::compile_to_rust(&bytes)
        .expect("compile sensor.exig");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("sensor_grammar.rs"), source)
        .expect("write generated grammar");
}
```

In the firmware crate:

```rust
fn sensor_grammar() -> exi::Result<exi::grammar::GrammarSet> {
    include!(concat!(env!("OUT_DIR"), "/sensor_grammar.rs"))
}
```

Call `sensor_grammar()` once, then attach the result to the firmware's string pool
and reuse the attachment. The target constructs the regular owned grammar tables
and still needs an allocator. The generated Rust does not embed the `.exig` file
or require the `.exig` decoder or SHA-256 at runtime.

## An example

This schema has one global element with a required enumerated attribute, an optional
`dateTime` attribute, and one or more `temp` children:

```xml
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:s"
           elementFormDefault="qualified">
  <xs:element name="reading">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="temp" type="xs:int" maxOccurs="unbounded"/>
      </xs:sequence>
      <xs:attribute name="unit" use="required">
        <xs:simpleType><xs:restriction base="xs:string">
          <xs:enumeration value="C"/><xs:enumeration value="F"/>
        </xs:restriction></xs:simpleType>
      </xs:attribute>
      <xs:attribute name="at" type="xs:dateTime"/>
    </xs:complexType>
  </xs:element>
</xs:schema>
```

Its `.exig` file is 109 bytes. Written out by `exi exig` and formatted with
`xmllint --format`, it is:

```xml
<grammars xmlns="urn:exig:draft-1" builtinTypes="true">
  <strings>
    <uri value="">
      <local>at</local>
      <local>unit</local>
    </uri>
    <uri value="urn:s">
      <local>reading</local>
      <local>temp</local>
    </uri>
  </strings>
  <simpleTypes>
    <enumeration base="39">
      <value>C</value>
      <value>F</value>
    </enumeration>
  </simpleTypes>
  <elementSets/>
  <attributes>
    <attribute local="0" type="18" uri="0"/>
    <attribute local="1" required="true" type="46" uri="0"/>
  </attributes>
  <types>
    <type>
      <attributes>0 1</attributes>
      <content>
        <state>
          <se element="1" next="1"/>
        </state>
        <state>
          <se element="1"/>
          <ee/>
        </state>
      </content>
    </type>
  </types>
  <elements>
    <element local="0" type="46" uri="4"/>
    <element local="1" type="29" uri="4"/>
  </elements>
  <global>0</global>
</grammars>
```

How to read it:

- URI 4 is `urn:s`, after the four URIs every EXI string table starts with.
- Simple type 18 is `xs:dateTime` and 39 is `xs:string`, both built-in. Simple type 46,
  the first one listed, is the `unit` enumeration.
- Type 29 is `xs:int`, which is built-in. Type 46, the first one listed, is the
  anonymous type of `reading`.
- `reading` uses attributes 0 (`at`) and 1 (`unit`) from the pool. Its attribute states
  are derived from them, so only its content is listed.
- The content is one or more `temp`, then the end of the element.

## Numbers

Everything is referred to by its number: its position in a list, starting at 0.

### Names

A name is a URI number and a local-name number within that URI. Both are numbers into
the initial string table EXI builds for the grammars (EXI §7.3.1, Appendix D).

The table holds:

- the entries every EXI string table has: the URIs `""`, the XML namespace, the XSI
  namespace and the XML Schema namespace, with their local names;
- the URIs and local names listed under `<strings>`.

Each partition is sorted, as EXI sorts it. A `<uri>` under `<strings>` may be one of the
fixed URIs; its local names then join that URI's partition.

### Built-in types

With `builtinTypes="true"`, the XML Schema built-in types come first and are not listed:

- **Types 0–45** are the XML Schema built-in types, sorted by local name in code-point
  order: `ENTITIES` is 0, `anyType` is 12, `int` is 29, `string` is 39, and
  `unsignedShort` is 45.
- **Simple types 0–45** are their datatypes in the same order. The exception is 12,
  `anyType`, which has no datatype; simple type 12 is the string type of its character
  content.
- Their grammars and derivations are those EXI gives them.

The lists in the document continue from 46. The reference table is the EXIG schema's
own grammars (`src/grammar/exig_tables.rs`).

A writer sets `builtinTypes` whenever the grammars start with exactly these types.

### Order

Everything is in one canonical order, so equal grammars give equal documents.

**Types:**
1. the built-in types;
2. then the other named types, sorted by URI then local name;
3. then anonymous types, in the order a depth-first walk first reaches them.

The walk visits the global elements sorted by local name then URI, then the named types.

**Elements and simple types:** in the order the same walk first reaches them.

**The attribute pool:** sorted by local name, then URI, then type, then `required`.

**Productions in a state:** in event-code order (EXI §8.5.4.3).

## The document

`<grammars>` has these children, in this order.

### `<strings>`

The initial string table beyond its fixed entries: a `<uri value="…">` for each URI,
holding a `<local>` for each local name. See [Names](#names).

### `<simpleTypes>`

The simple types after the built-in ones. Each is one element naming its EXI datatype
representation (EXI §7.1, §7.2):

- `string`, with an optional `<charset>` child: a restricted character set (EXI
  §7.1.10.1), with its characters sorted;
- `boolean`, and `booleanPattern` for a boolean with a pattern facet;
- `integer`, `unsignedInteger`, and `boundedInteger` with `minInclusive` and
  `maxInclusive`;
- `decimal` and `float`;
- `hexBinary` and `base64Binary`;
- `dateTime`, `date`, `time`, `gYear`, `gYearMonth`, `gMonth`, `gMonthDay` and `gDay`;
- `list` with `itemType`, a simple type number;
- `enumeration` with `base`, the simple type its values are of, holding a `<value>`
  for each value in ordinal order.

### `<elementSets>`

Lists of element numbers that `<seSet>` refers to. Each `<set>` is a space-separated
list. A writer makes a set for any run of three or more `<se>` productions in one state
that go to the same state. It writes each distinct run once, numbered in the order
first used.

### `<attributes>`

The attribute pool. Each `<attribute>` is an attribute use, with:

- `uri` and `local`, its name;
- `type`, its simple type;
- `required`, when it is required.

### `<attributeSet>`

This element is optional. It holds the attribute uses that many types share, as a list
of pool numbers. A type includes them with `set="0"`.

A writer gives a set when the uses found in the most types are in at least three types
and there are at least two of them. A type that has all of them uses the set.

### `<types>`

The types after the built-in ones. Each `<type>` has these attributes:

- `uri` and `local`, the name of a named type. Anonymous types have neither.
- `base`, the named type this one derives from.
- `castable`: in strict streams, elements of this type take `AT(xsi:type)` (EXI
  §8.5.4.4.1).

The types `xsi:type` may name for a type are the named types derived from it, directly
or not. They follow from `base`; there are no cast lists.

A `<type>` has these children:

1. `<attributes>`: its attribute uses, as pool numbers in ascending order. With `set="0"`
   it also has the shared `<attributeSet>`; its own list then holds only the rest.
2. `<anyAttribute>`, once per attribute wildcard: `AT(*)` without `uri`, `AT(uri:*)`
   with one.
3. One of:
   - `<empty/>`: the content is only `EE`;
   - `<scalar type="n"/>`: the content is `CH` of simple type `n`, then `EE`;
   - `<content>`: the content states, given state by state.

### `<elements>`

The element declarations. Each `<element>` has:

- `uri` and `local`, its name;
- `type`, its type number;
- `nillable`, when it is nillable.

### `<global>`

The global elements, one element number each.

With `allGlobal="true"` on `<grammars>`, there are none: every element is global, in
order of local name then URI.

### Attributes of `<grammars>`

- `builtinTypes`: see [Built-in types](#built-in-types).
- `allGlobal`: see [`<global>`](#global).

## Type grammars

A type's grammar is derived from its attribute uses, its wildcards and its content
states.

### Attribute states

These follow EXI §8.5.4.1.3.1. With `k` attribute uses in pool order, there is one
attribute state per use. Attribute state `i` offers, in this order:

1. `AT` of use `i` and of each later use, up to and including the first required one.
   Each goes to the state after that use.
2. Each wildcard, going back to state `i`.
3. When no use from `i` on is required, the productions of the content start.

Without wildcards, the last use goes to content state 0. With a wildcard there is one
more attribute state, which the last use goes to; it offers the wildcards (back to
itself) and the content start's productions.

### Content states

Inside `<content>`, the `<state>` elements are the content states. They are numbered
from 0, the content start, in breadth-first order from it: in the order the productions,
taken in order, first reach them.

Each state holds its productions in event-code order:

| Production | Event | Attributes |
|---|---|---|
| `se` | `SE(qname)` | `element` |
| `seSet` | `SE` for each element of a set, in its order, all to the same state | `set` |
| `seUri` | `SE(uri:*)` | `uri` |
| `seAny` | `SE(*)` | |
| `ee` | `EE` | |
| `ch` | `CH` | `type`, a simple type |
| `at` | `AT(qname)` | `uri`, `local`, `type` |
| `atUri` | `AT(uri:*)` | `uri` |
| `atAny` | `AT(*)` | |

Every production but `ee` has `next`, the content state it goes to. Without `next`, it
goes to the state it is in.

### Grammars that don't fit

A type whose grammar is not attribute states followed by content states lists no
`<attributes>` or `<anyAttribute>`. Its `<content>` then gives every state, with the
`at`, `atUri` and `atAny` productions written out. A type is also written this way when
its attribute uses are not in pool order.

### The final numbering

The type's grammar is numbered breadth-first from its first state, following
productions in order. States nothing reaches are left out.

In the example, `reading` has uses `at` (optional) and `unit` (required):

| State | Productions |
|---|---|
| 0 | `AT(at)` → 1, `AT(unit)` → 2 |
| 1 | `AT(unit)` → 2 |
| 2 | `SE(temp)` → 3 |
| 3 | `SE(temp)` → 3, `EE` |

States 2 and 3 are content states 0 and 1.

## Non-strict grammars

Only the strict grammars are written. A reader adds the productions for non-strict
streams as EXI §8.5.4.4 specifies.

## Checks

A reader checks a file before using its grammars. A file fails when:

- a number refers to something that doesn't exist;
- productions are not in event-code order, or repeat;
- a facet doesn't fit its representation;
- a type uses an attribute use twice, or uses `set="0"` without an `<attributeSet>`;
- an anonymous type has a `base`, or the bases form a cycle;
- a type has no content state.

It also fails when the grammars expand to more than 2²⁴ productions in all.

## Writing

`GrammarSet::to_exig` writes any schema-informed grammar set, whether it was compiled
from XSD, built by hand or read from an `.exig` file. It fails when the `xsi:type` casts
can't be expressed as one `base` per named type. For each named type, the writer takes
as its base the type with the fewest casts among those that may be cast to it.

The writer has one way to write each event, so equal grammars give equal bytes.
EXIficient, encoding the same EXIG document under the same options, gives the same
bytes.
