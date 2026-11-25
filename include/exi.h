/* exi.h: libexi layer 1, the C ABI (feature "capi").
 *
 * Generated from src/capi/mod.rs by cbindgen (cbindgen.toml). Do not edit: change the
 * Rust and generate again. The conventions (status codes, strings, config structs,
 * threads, lifetimes) are in the documentation of src/capi/mod.rs.
 *
 * Layer 2, the document tree, is in exi_dom.h.
 */

#ifndef EXI_H
#define EXI_H

#include <stddef.h>
#include <stdint.h>
typedef struct exi_strings exi_strings;
typedef struct exi_grammar_set exi_grammar_set;
typedef struct exi_reader exi_reader;
typedef struct exi_writer exi_writer;

/*
 The ABI version: major in the high 16 bits, minor in the low 16.
 */
#define EXI_ABI_MAJOR 0

#define EXI_ABI_MINOR 11

/*
 Feature bits from [`exi_features`].
 */
#define EXI_FEAT_XML_TEXT (1 << 1)

#define EXI_FEAT_SELF_CONTAINED (1 << 4)

/*
 Bit-packed bodies (`EXI_CODING_BIT_PACKED`).
 */
#define EXI_FEAT_BIT_PACKED (1 << 7)

/*
 Pre-compression and compression (`EXI_CODING_PRE_COMPRESSION`,
 `EXI_CODING_COMPRESSION`).
 */
#define EXI_FEAT_DEFLATE (1 << 0)

/*
 Layer 2, the document tree (`include/exi_dom.h`).
 */
#define EXI_FEAT_DOM (1 << 8)

#define EXI_NO_ID UINT32_MAX

#define EXI_V_NONE 0

#define EXI_V_STRING 1

#define EXI_V_BOOL 2

#define EXI_V_INT 3

#define EXI_V_DECIMAL 4

#define EXI_V_FLOAT 5

#define EXI_V_BINARY 6

#define EXI_V_DATETIME 7

#define EXI_V_QNAME 8

#define EXI_V_LIST 9

#define EXI_V_ENUM 10

/*
 `text` holds the value's digits: a `BigInt` or `BigDecimal`.
 */
#define EXI_VF_TEXT_ONLY (1 << 0)

#define EXI_VF_LEXICAL (1 << 1)

#define EXI_VF_UNTYPED (1 << 2)

#define EXI_VF_HEX (1 << 3)

#define EXI_VF_BOOL_DIGIT (1 << 4)

#define EXI_DATETIME_G_YEAR 1

#define EXI_DATETIME_G_YEAR_MONTH 2

#define EXI_DATETIME_DATE 3

#define EXI_DATETIME_DATE_TIME 4

#define EXI_DATETIME_G_MONTH 5

#define EXI_DATETIME_G_MONTH_DAY 6

#define EXI_DATETIME_G_DAY 7

#define EXI_DATETIME_TIME 8

#define EXI_EV_SD 1

#define EXI_EV_ED 2

#define EXI_EV_SE 3

#define EXI_EV_EE 4

#define EXI_EV_AT 5

#define EXI_EV_CH 6

#define EXI_EV_NS 7

#define EXI_EV_CM 8

#define EXI_EV_PI 9

#define EXI_EV_DT 10

#define EXI_EV_ER 11

#define EXI_EV_SC 12

#define EXI_CODING_BIT_PACKED 0

#define EXI_CODING_BYTE_ALIGNED 1

#define EXI_CODING_PRE_COMPRESSION 2

#define EXI_CODING_COMPRESSION 3

#define EXI_OPT_STRICT (1 << 0)

#define EXI_OPT_FRAGMENT (1 << 1)

#define EXI_OPT_SELF_CONTAINED (1 << 2)

#define EXI_PRESERVE_DTD (1 << 0)

#define EXI_PRESERVE_PREFIXES (1 << 1)

#define EXI_PRESERVE_LEXICAL (1 << 2)

#define EXI_PRESERVE_COMMENTS (1 << 3)

#define EXI_PRESERVE_PIS (1 << 4)

#define EXI_SCHEMA_ID_ABSENT 0

#define EXI_SCHEMA_ID_NIL 1

#define EXI_SCHEMA_ID_BUILTIN 2

#define EXI_SCHEMA_ID_VALUE 3

/*
 "Not set" for the optional numeric options.
 */
#define EXI_ABSENT UINT64_MAX

#define EXI_FORMAT_EXI 0

#define EXI_FORMAT_XML 1

#define EXI_SEQ_SINGLE 0

#define EXI_SEQ_STREAMS 1

#define EXI_SEQ_FRAMES 2

#define EXI_RD_START_TAG (1 << 1)

#define EXI_RD_XML_DOCTYPE (1 << 3)

#define EXI_RD_XML_PREDEFINED_ONLY (1 << 4)

#define EXI_FEED_FINAL (1 << 0)

#define EXI_FEED_END_FRAME (1 << 1)

#define EXI_HDR_COOKIE (1 << 0)

#define EXI_HDR_NO_OPTIONS (1 << 1)

#define EXI_WR_XML_DECLARATION (1 << 3)

/*
 Canonical EXI (EXI C14N); refused with `EXI_HDR_COOKIE` and
 `EXI_CODING_COMPRESSION`.
 */
#define EXI_WR_CANONICAL (1 << 4)

/*
 With `EXI_WR_CANONICAL`: date-times in UTC (C14N `utcTime`).
 */
#define EXI_WR_UTC_TIME (1 << 5)

/*
 Bytes a digest takes with its NUL: `sha256:`, 64 hex digits, NUL.
 */
#define EXI_DIGEST_LEN 72

/*
 A status code.
 */
typedef int32_t exi_status;

typedef struct exi_str {
  const char *ptr;
  size_t len;
} exi_str;

/*
 Options. `exi_options_init` fills the EXI defaults.
 */
typedef struct exi_options {
  uint32_t size;
  uint32_t coding;
  uint32_t flags;
  uint32_t preserve;
  uint64_t block_size;
  uint64_t value_max_length;
  uint64_t value_partition_capacity;
  uint32_t schema_id_kind;
  uint32_t _pad;
  struct exi_str schema_id;
} exi_options;

/*
 Limits; a zero field means the default.
 */
typedef struct exi_limits {
  uint32_t size;
  uint32_t max_depth;
  uint32_t max_attributes;
  uint32_t max_integer_octets;
  uint32_t max_names;
  uint32_t max_list_items;
  uint32_t max_options_bytes;
  uint32_t max_entity_depth;
  uint64_t max_value_chars;
  uint64_t max_binary_bytes;
  uint64_t max_string_table_bytes;
  uint64_t max_learned_productions;
  uint64_t max_expanded_bytes;
} exi_limits;

typedef struct exi_strings_config {
  uint32_t size;
  /*
   16 (0 = default) or 32.
   */
  uint32_t hash_bits;
  /*
   0 = 16 KiB.
   */
  uint32_t chunk_bytes;
  uint32_t _pad;
  /*
   0 = no limit.
   */
  uint64_t max_bytes;
  uint64_t max_strings;
} exi_strings_config;

typedef struct exi_reader_config {
  uint32_t size;
  uint32_t flags;
  uint32_t sequence;
  uint32_t format;
  const struct exi_options *options;
  /*
   NULL: schema-less.
   */
  exi_grammar_set *grammars;
  const struct exi_limits *limits;
  /*
   NULL: private string tables.
   */
  exi_strings *strings;
  /*
   Ring size: 0 = 4096; otherwise 64, 256, 1024, 4096, 16384 or
   65536.
   */
  uint32_t input_buffer_size;
  uint32_t _pad;
} exi_reader_config;

/*
 A name. `uri_id`/`local_id` are the session numbers and
 `global_uri`/`global_local` the global ids; `EXI_NO_ID` when
 unknown.
 */
typedef struct exi_name {
  struct exi_str uri;
  struct exi_str local;
  struct exi_str prefix;
  uint32_t uri_id;
  uint32_t local_id;
  uint32_t global_uri;
  uint32_t global_local;
} exi_name;

typedef struct exi_decimal {
  uint32_t negative;
  uint32_t fraction_digits;
  uint64_t integral;
  uint64_t fraction;
} exi_decimal;

typedef struct exi_float {
  int64_t mantissa;
  int32_t exponent;
  uint32_t _pad;
} exi_float;

typedef struct exi_bytes {
  const uint8_t *ptr;
  size_t len;
} exi_bytes;

/*
 `kind` is an `EXI_DATETIME_*` constant.
 */
typedef struct exi_datetime {
  uint32_t kind;
  int32_t tz_minutes;
  int64_t year;
  uint8_t month;
  uint8_t day;
  uint8_t hour;
  uint8_t minute;
  uint8_t second;
  uint8_t has_tz;
  uint8_t has_frac;
  uint8_t _pad;
  uint32_t frac_digits;
  uint64_t frac;
} exi_datetime;

typedef struct exi_list {
  const struct exi_value *items;
  size_t count;
} exi_list;

typedef union exi_value_u {
  int32_t boolean;
  int64_t integer;
  struct exi_decimal dec;
  struct exi_float flt;
  struct exi_bytes binary;
  struct exi_datetime datetime;
  struct exi_name qname;
  struct exi_list list;
  uint32_t enum_ordinal;
} exi_value_u;

/*
 A value: `type` is an `EXI_V_*` constant and selects the union
 member. `text` is always set for strings and enumerations, and for
 any value when `EXI_VF_LEXICAL` is set.
 */
typedef struct exi_value {
  uint32_t type;
  uint32_t flags;
  struct exi_str text;
  union exi_value_u u;
} exi_value;

typedef struct exi_nsdecl {
  struct exi_str prefix;
  struct exi_str uri;
  uint32_t local_element_ns;
  uint32_t _pad;
} exi_nsdecl;

/*
 One event. Only the fields listed for its type are set:

 | Type | Fields |
 |---|---|
 | SE, EE | `name` |
 | AT | `name`, `value` |
 | CH | `value` |
 | NS | `ns` (one entry) |
 | CM | `value.text` |
 | PI | `name.local` (target), `value.text` (data) |
 | DT | `name.local`, `pubid`, `sysid`, `value.text` (internal subset) |
 | ER | `name.local` |
 */
typedef struct exi_event {
  uint32_t type;
  uint32_t depth;
  struct exi_name name;
  struct exi_value value;
  struct exi_str pubid;
  struct exi_str sysid;
  const struct exi_nsdecl *ns;
  size_t ns_count;
} exi_event;

/*
 Push callbacks; any may be NULL. Non-zero from a callback stops the
 run.
 */
typedef struct exi_handlers {
  uint32_t size;
  int (*start_document)(void *user);
  int (*end_document)(void *user);
  int (*start_element)(void *user, const struct exi_name *name);
  int (*end_element)(void *user, const struct exi_name *name);
  int (*attribute)(void *user, const struct exi_name *name, const struct exi_value *value);
  int (*characters)(void *user, const struct exi_value *value);
  int (*namespace_decl)(void *user, const struct exi_nsdecl *ns);
  int (*comment)(void *user, struct exi_str text);
  int (*processing_instruction)(void *user, struct exi_str target, struct exi_str data);
  int (*doctype)(void *user,
                 struct exi_str name,
                 struct exi_str public_id,
                 struct exi_str system_id,
                 struct exi_str text);
  int (*entity_reference)(void *user, struct exi_str name);
  int (*self_contained)(void *user);
} exi_handlers;

/*
 Called with output octets; a status other than `EXI_OK` stops the
 writer with that status.
 */
typedef exi_status (*exi_write_fn)(void *user, const uint8_t *data, size_t len);

typedef struct exi_writer_config {
  uint32_t size;
  uint32_t flags;
  uint32_t header;
  uint32_t sequence;
  uint32_t format;
  uint32_t _pad;
  const struct exi_options *options;
  exi_grammar_set *grammars;
  /*
   NULL: accumulate; see `exi_writer_take`.
   */
  exi_write_fn write;
  void *write_user;
  const struct exi_limits *limits;
  exi_strings *strings;
} exi_writer_config;

#define EXI_OK 0

#define EXI_END 1

#define EXI_NEED_INPUT 2

#define EXI_E_ARG -1

#define EXI_E_STATE -2

#define EXI_E_NOMEM -3

#define EXI_E_IO -4

#define EXI_E_HEADER -5

#define EXI_E_GRAMMAR -6

#define EXI_E_VALUE -7

#define EXI_E_TRUNCATED -8

#define EXI_E_LIMIT -9

#define EXI_E_SCHEMA -10

#define EXI_E_UNSUPPORTED -11

#define EXI_E_XML -12

#define EXI_E_INTERNAL -13

#define EXI_E_ABORTED -14

#define EXI_E_BUFFER_FULL -15

#define EXI_E_UNENCODABLE -16















#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/*
 `(EXI_ABI_MAJOR << 16) | EXI_ABI_MINOR`.

 ```
 assert_eq!(exi::capi::exi_abi_version(), (exi::capi::EXI_ABI_MAJOR << 16) | exi::capi::EXI_ABI_MINOR);
 ```
 */
uint32_t exi_abi_version(void);

/*
 The crate version, NUL-terminated, static.
 */
const char *exi_version_string(void);

/*
 The `EXI_FEAT_*` bits of this build: `EXI_FEAT_SELF_CONTAINED`
 always, and one bit for each of the features `xml`, `dom`,
 `bit-packed` and `compression`.

 ```
 use exi::capi::*;
 let f = exi_features();
 assert_ne!(f & EXI_FEAT_SELF_CONTAINED, 0);
 assert_ne!(f & EXI_FEAT_DOM, 0);
 assert_eq!(f & EXI_FEAT_BIT_PACKED != 0, cfg!(feature = "bit-packed"));
 assert_eq!(f & EXI_FEAT_DEFLATE != 0, cfg!(feature = "compression"));
 ```
 */
uint64_t exi_features(void);

/*
 A static, NUL-terminated description of a status: `"ok"`, `"end"`,
 `"need input"`, the `ErrorKind` in words for errors, `"unknown
 status"` otherwise.

 ```
 use exi::capi::*;
 use std::ffi::CStr;
 let s = |c| unsafe { CStr::from_ptr(exi_status_string(c)) }.to_str().unwrap();
 assert_eq!(s(EXI_OK), "ok");
 assert_eq!(s(EXI_E_UNENCODABLE), "event fits no production");
 assert_eq!(s(12345), "unknown status");
 ```
 */
const char *exi_status_string(exi_status s);

/*
 Fills `*o` with EXI's defaults and sets `o->size`. NULL is a no-op.

 # Safety

 `o` is NULL or points to writable memory for an `exi_options`.
 */
void exi_options_init(struct exi_options *o);

/*
 Checks a combination (see [`crate::options`]). On error, `*why`
 (when not NULL) gets a static NUL-terminated reason.

 # Safety

 `o` points to an `exi_options`; `why` is NULL or writable.

 ```
 use exi::capi::*;
 unsafe {
     let mut o = core::mem::zeroed::<exi_options>();
     exi_options_init(&mut o);
     assert_eq!((o.coding, o.block_size, o.value_max_length), (EXI_CODING_BIT_PACKED, 1_000_000, EXI_ABSENT));
     assert_eq!(exi_options_validate(&o, core::ptr::null_mut()), EXI_OK);
     o.flags = EXI_OPT_STRICT;
     o.preserve = EXI_PRESERVE_PREFIXES;
     let mut why = core::ptr::null();
     assert_eq!(exi_options_validate(&o, &mut why), EXI_E_ARG);
     assert!(!why.is_null());
     o.flags = 0; o.preserve = 0; o.coding = 9;                 // unknown constant
     assert_eq!(exi_options_validate(&o, core::ptr::null_mut()), EXI_E_ARG);
 }
 ```
 */
exi_status exi_options_validate(const struct exi_options *o,
                                const char **why);

/*
 Fills `*l` with the default limits and sets `l->size`. NULL is a
 no-op.

 # Safety

 `l` is NULL or writable.
 */
void exi_limits_init(struct exi_limits *l);

/*
 New global string tables; NULL config: defaults. NULL on a bad
 config.

 # Safety

 `cfg` is NULL or points to an `exi_strings_config` of at least
 `cfg->size` bytes.
 */
exi_strings *exi_strings_new(const struct exi_strings_config *cfg);

/*
 # Safety

 `s` is NULL or from `exi_strings_new`, and no reader or writer still
 uses it.
 */
void exi_strings_free(exi_strings *s);

/*
 The schema-less grammar set: static; retain and release do nothing
 to it.
 */
exi_grammar_set *exi_grammar_set_schemaless(void);

/*
 Adds a reference; returns `g`.

 # Safety

 `g` is a live grammar set.
 */
exi_grammar_set *exi_grammar_set_retain(exi_grammar_set *g);

/*
 Drops a reference; the set is freed at zero. NULL is a no-op.

 # Safety

 `g` is NULL or a live grammar set the caller holds a reference to.
 */
void exi_grammar_set_release(exi_grammar_set *g);

/*
 The fixed numbers of a declared name; `EXI_E_ARG` when it is not an
 initial entry.

 # Safety

 `g` is live; `uri_id` and `local_id` are writable.
 */
exi_status exi_grammar_set_name_id(const exi_grammar_set *g,
                                   struct exi_str uri,
                                   struct exi_str local,
                                   uint32_t *uri_id,
                                   uint32_t *local_id);

/*
 A grammar set from an `.exig` file, with one reference (release it
 with `exi_grammar_set_release`). NULL `out` or data is `EXI_E_ARG`;
 a file that fails its checks returns that error, and `*out` is left
 unchanged.

 ```
 use exi::capi::*;
 # #[cfg(feature = "xsd")] {
 let xsd = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a"/></xs:schema>"#;
 let file = exi::grammar::GrammarSet::from_xsd(xsd).unwrap().to_exig().unwrap();
 let mut g: *mut exi_grammar_set = core::ptr::null_mut();
 unsafe {
     assert_eq!(exi_grammar_set_from_exig(file.as_ptr(), file.len(), &mut g), EXI_OK);
     let mut digest = [0 as core::ffi::c_char; EXI_DIGEST_LEN];
     assert_eq!(exi_grammar_set_digest(g, digest.as_mut_ptr()), EXI_OK);
     let d = core::ffi::CStr::from_ptr(digest.as_ptr()).to_str().unwrap();
     assert_eq!(d, exi::grammar::GrammarSet::from_exig(&file).unwrap().digest().unwrap());

     extern "C" fn collect(user: *mut core::ffi::c_void, data: *const u8, len: usize) -> exi_status {
         let v = unsafe { &mut *(user as *mut Vec<u8>) };
         v.extend_from_slice(unsafe { core::slice::from_raw_parts(data, len) });
         EXI_OK
     }
     let mut again: Vec<u8> = Vec::new();
     assert_eq!(exi_grammar_set_to_exig(g, Some(collect), &mut again as *mut _ as *mut _), EXI_OK);
     assert_eq!(again, file);
     exi_grammar_set_release(g);

     assert_eq!(exi_grammar_set_from_exig(b"\x80".as_ptr(), 1, &mut g), EXI_E_HEADER);
     assert_eq!(exi_grammar_set_to_exig(exi_grammar_set_schemaless(), Some(collect), &mut again as *mut _ as *mut _), EXI_E_ARG);
 }
 # }
 ```

 # Safety

 `data` is readable for `len` bytes; `out` is writable.
 */
exi_status exi_grammar_set_from_exig(const uint8_t *data,
                                     size_t len,
                                     exi_grammar_set **out);

/*
 Writes the set's `.exig` file through `write` (called one or more
 times with `user`); a status other than `EXI_OK` from it stops the
 writing with that status. Schema-less sets have no `.exig` form:
 `EXI_E_ARG`.

 # Safety

 `g` is live.
 */
exi_status exi_grammar_set_to_exig(const exi_grammar_set *g,
                                   exi_status (*write)(void*, const uint8_t*, size_t),
                                   void *user);

/*
 The set's digest (`sha256:` and 64 lowercase hex digits,
 NUL-terminated) into `out`, which has room for `EXI_DIGEST_LEN`
 bytes. Schema-less sets: `EXI_E_ARG`.

 # Safety

 `g` is live; `out` is writable for `EXI_DIGEST_LEN` bytes.
 */
exi_status exi_grammar_set_digest(const exi_grammar_set *g, char *out);

/*
 A new reader; NULL on a bad config (call nothing else to find out
 why: check the config with `exi_options_validate`).

 # Safety

 `cfg` points to an `exi_reader_config` of at least `cfg->size` bytes
 whose pointers are NULL or valid for the reader's life (options and
 limits are copied).

 ```
 use exi::capi::*;
 unsafe {
     let mut o = core::mem::zeroed::<exi_options>();
     exi_options_init(&mut o);
     o.coding = EXI_CODING_BYTE_ALIGNED;
     let mut cfg = core::mem::zeroed::<exi_reader_config>();
     cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
     cfg.options = &o;
     let r = exi_reader_new(&cfg);
     assert!(!r.is_null());
     let input = [0x80u8, 0x01, 0x02, b'a', 0x00];
     let mut taken = 0;
     assert_eq!(exi_reader_feed(r, input.as_ptr().cast(), input.len(), EXI_FEED_FINAL, &mut taken), EXI_OK);
     assert_eq!(taken, 5);
     let mut ev = core::mem::zeroed::<exi_event>();
     let mut types = Vec::new();
     loop {
         match exi_reader_next(r, &mut ev) {
             EXI_OK => types.push(ev.r#type),
             EXI_END => break,
             s => panic!("{s}"),
         }
     }
     assert_eq!(types, [EXI_EV_SD, EXI_EV_SE, EXI_EV_EE, EXI_EV_ED]);
     exi_reader_free(r);
     exi_reader_free(core::ptr::null_mut());                 // no-op
     cfg.input_buffer_size = 100;                            // not an allowed size
     assert!(exi_reader_new(&cfg).is_null());
 }
 ```
 */
exi_reader *exi_reader_new(const struct exi_reader_config *cfg);

/*
 # Safety

 `r` is NULL or a live reader.
 */
void exi_reader_free(exi_reader *r);

/*
 Copies as much of `data` as fits into the ring; `*accepted` (may be
 NULL) gets the count. `EXI_FEED_FINAL` and `EXI_FEED_END_FRAME` take
 effect only when all `len` octets were taken. `data` may be NULL
 when `len` is 0, to give flags alone.

 # Safety

 `r` is live; `data` is valid for `len` bytes.
 */
exi_status exi_reader_feed(exi_reader *r,
                           const void *data,
                           size_t len,
                           uint32_t flags,
                           size_t *accepted);

/*
 Free ring space for reading in place; `*len` gets its size.

 # Safety

 `r` is live; `len` is writable.
 */
uint8_t *exi_reader_buffer(exi_reader *r, size_t *len);

/*
 # Safety

 `r` is live; `n` is at most the length `exi_reader_buffer` gave.
 */
exi_status exi_reader_commit(exi_reader *r, size_t n);

/*
 `EXI_OK` with `*out` filled, `EXI_NEED_INPUT`, `EXI_END`, or an
 error.

 # Safety

 `r` is live; `out` is writable.
 */
exi_status exi_reader_next(exi_reader *r, struct exi_event *out);

/*
 The stream's options once the header is read, else NULL. Valid while
 the reader lives.

 # Safety

 `r` is live.
 */
const struct exi_options *exi_reader_options(const exi_reader *r);

/*
 The last error: its status; `*msg`, `*byte_offset`, `*event_index`
 when not NULL.

 # Safety

 `r` is live; out pointers are NULL or writable.
 */
exi_status exi_reader_error(const exi_reader *r,
                            const char **msg,
                            uint64_t *byte_offset,
                            uint64_t *event_index);

/*
 Runs the handlers (see the module documentation).

 # Safety

 `r` is live; `h` points to an `exi_handlers` of at least `h->size`
 bytes.

 ```
 use exi::capi::*;
 extern "C" fn stop(_: *mut core::ffi::c_void, _: *const exi_name) -> core::ffi::c_int { 1 }
 unsafe {
     let mut cfg = core::mem::zeroed::<exi_reader_config>();
     cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
     cfg.format = EXI_FORMAT_XML;
     let r = exi_reader_new(&cfg);
     exi_reader_feed(r, b"<a/>".as_ptr().cast(), 4, EXI_FEED_FINAL, core::ptr::null_mut());
     let mut h = core::mem::zeroed::<exi_handlers>();
     h.size = core::mem::size_of::<exi_handlers>() as u32;
     h.start_element = Some(stop);
     assert_eq!(exi_reader_run(r, &h, core::ptr::null_mut()), EXI_E_ABORTED);
     let mut ev = core::mem::zeroed::<exi_event>();
     assert_eq!(exi_reader_next(r, &mut ev), EXI_E_STATE);    // poisoned
     exi_reader_free(r);
 }
 ```
 */
exi_status exi_reader_run(exi_reader *r, const struct exi_handlers *h, void *user);

/*
 A new writer; NULL on a bad config.

 # Safety

 As `exi_reader_new`, for `exi_writer_config`.

 ```
 use exi::capi::*;
 unsafe {
     let mut o = core::mem::zeroed::<exi_options>();
     exi_options_init(&mut o);
     o.coding = EXI_CODING_BYTE_ALIGNED;
     let mut cfg = core::mem::zeroed::<exi_writer_config>();
     cfg.size = core::mem::size_of::<exi_writer_config>() as u32;
     cfg.options = &o;
     cfg.header = EXI_HDR_NO_OPTIONS;
     let w = exi_writer_new(&cfg);
     let s = |t: &str| exi_str { ptr: t.as_ptr().cast(), len: t.len() };
     assert_eq!(exi_writer_start_element(w, s(""), s("a")), EXI_OK);
     assert_eq!(exi_writer_end_element(w), EXI_OK);
     assert_eq!(exi_writer_end_document(w), EXI_OK);
     let (mut p, mut n) = (core::ptr::null(), 0);
     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
     assert_eq!(core::slice::from_raw_parts(p, n), [0x80, 0x01, 0x02, b'a', 0x00]);
     assert_eq!(exi_writer_end_element(w), EXI_E_STATE);
     exi_writer_free(w);
 }
 ```
 */
exi_writer *exi_writer_new(const struct exi_writer_config *cfg);

/*
 # Safety

 `w` is NULL or a live writer.
 */
void exi_writer_free(exi_writer *w);

/*
 Any event, as the reader returns them.

 # Safety

 `w` is live; `ev` points to a filled `exi_event`.
 */
exi_status exi_writer_event(exi_writer *w, const struct exi_event *ev);

/*
 # Safety

 `w` is live; the strings are valid.
 */
exi_status exi_writer_start_element(exi_writer *w, struct exi_str uri, struct exi_str local);

/*
 # Safety

 `w` is live; the strings are valid.
 */
exi_status exi_writer_attribute(exi_writer *w,
                                struct exi_str uri,
                                struct exi_str local,
                                struct exi_str text);

/*
 # Safety

 `w` is live; `text` is valid.
 */
exi_status exi_writer_text(exi_writer *w, struct exi_str text);

/*
 # Safety

 `w` is live.
 */
exi_status exi_writer_end_element(exi_writer *w);

/*
 ED, padding, flush.

 # Safety

 `w` is live.
 */
exi_status exi_writer_end_document(exi_writer *w);

/*
 `*pending_bits` (may be NULL) gets the bits not yet written (0 when
 byte-aligned).

 # Safety

 `w` is live.
 */
exi_status exi_writer_flush(exi_writer *w, uint32_t *pending_bits);

/*
 `EXI_SEQ_FRAMES`: after a top-level element ends, the octets since
 the previous frame. Valid until the next call on the writer.

 # Safety

 `w` is live; `data` and `len` are writable.
 */
exi_status exi_writer_end_frame(exi_writer *w, const uint8_t **data, size_t *len);

/*
 With `write` NULL: the output accumulated so far, valid until the
 next call; the buffer is emptied.

 # Safety

 `w` is live; `data` and `len` are writable.
 */
exi_status exi_writer_take(exi_writer *w, const uint8_t **data, size_t *len);

/*
 The last error's status and message.

 # Safety

 `w` is live; `msg` is NULL or writable.
 */
exi_status exi_writer_error(const exi_writer *w, const char **msg);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* EXI_H */
