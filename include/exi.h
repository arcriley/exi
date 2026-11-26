/* exi.h: libexi layer 1, the C ABI (feature "capi").
 *
 * Kept in step with src/capi/mod.rs by hand; tests/c_api.rs checks every declaration,
 * value and layout against it. The conventions (status codes, strings, config structs,
 * threads, lifetimes) are in the documentation of that module.
 *
 * Layer 2, the document tree, is in exi_dom.h.
 */
#ifndef EXI_H
#define EXI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef EXI_API
#define EXI_API
#endif

/* ----- status codes ----- */

typedef int32_t exi_status;

#define EXI_OK 0
#define EXI_END 1
#define EXI_NEED_INPUT 2
#define EXI_E_ARG (-1)
#define EXI_E_STATE (-2)
#define EXI_E_NOMEM (-3)
#define EXI_E_IO (-4)
#define EXI_E_HEADER (-5)
#define EXI_E_GRAMMAR (-6)
#define EXI_E_VALUE (-7)
#define EXI_E_TRUNCATED (-8)
#define EXI_E_LIMIT (-9)
#define EXI_E_SCHEMA (-10)
#define EXI_E_UNSUPPORTED (-11)
#define EXI_E_XML (-12)
#define EXI_E_INTERNAL (-13)
#define EXI_E_ABORTED (-14)
#define EXI_E_BUFFER_FULL (-15)
#define EXI_E_UNENCODABLE (-16)

/* ----- version and features ----- */

#define EXI_ABI_MAJOR 0
#define EXI_ABI_MINOR 12

#define EXI_FEAT_DEFLATE (1 << 0)
#define EXI_FEAT_XML_TEXT (1 << 1)
#define EXI_FEAT_SELF_CONTAINED (1 << 4)
#define EXI_FEAT_BIT_PACKED (1 << 7)
#define EXI_FEAT_DOM (1 << 8)

/* ----- strings, names and values ----- */

/* UTF-8. Strings libexi returns also have a NUL at ptr[len]. */
typedef struct exi_str {
    const char *ptr;
    size_t len;
} exi_str;

typedef struct exi_bytes {
    const uint8_t *ptr;
    size_t len;
} exi_bytes;

#define EXI_NO_ID UINT32_MAX

typedef struct exi_name {
    exi_str uri;
    exi_str local;
    exi_str prefix;
    uint32_t uri_id;        /* session numbers, or EXI_NO_ID */
    uint32_t local_id;
    uint32_t global_uri;    /* global ids, or EXI_NO_ID */
    uint32_t global_local;
} exi_name;

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

typedef struct exi_datetime {
    uint32_t kind;          /* EXI_DATETIME_* */
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

typedef struct exi_value exi_value;

typedef struct exi_list {
    const exi_value *items;
    size_t count;
} exi_list;

typedef union exi_value_u {
    int32_t boolean;
    int64_t integer;
    exi_decimal dec;
    exi_float flt;
    exi_bytes binary;
    exi_datetime datetime;
    exi_name qname;
    exi_list list;
    uint32_t enum_ordinal;
} exi_value_u;

/* `type` (EXI_V_*) selects the member of `u`. */
struct exi_value {
    uint32_t type;
    uint32_t flags;         /* EXI_VF_* */
    exi_str text;
    exi_value_u u;
};

/* ----- events ----- */

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

typedef struct exi_nsdecl {
    exi_str prefix;
    exi_str uri;
    uint32_t local_element_ns;
    uint32_t _pad;
} exi_nsdecl;

typedef struct exi_event {
    uint32_t type;          /* EXI_EV_* */
    uint32_t depth;
    exi_name name;
    exi_value value;
    exi_str pubid;
    exi_str sysid;
    const exi_nsdecl *ns;
    size_t ns_count;
} exi_event;

/* ----- options and limits ----- */

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

/* "Not set" for the optional numeric options. */
#define EXI_ABSENT UINT64_MAX

/* Config structs start with `size` = sizeof the struct; zero fields mean the default. */
typedef struct exi_options {
    uint32_t size;
    uint32_t coding;        /* EXI_CODING_* */
    uint32_t flags;         /* EXI_OPT_* */
    uint32_t preserve;      /* EXI_PRESERVE_* */
    uint64_t block_size;
    uint64_t value_max_length;
    uint64_t value_partition_capacity;
    uint32_t schema_id_kind; /* EXI_SCHEMA_ID_* */
    uint32_t _pad;
    exi_str schema_id;
} exi_options;

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

/* ----- handles ----- */

typedef struct exi_strings exi_strings;
typedef struct exi_grammar_set exi_grammar_set;
typedef struct exi_reader exi_reader;
typedef struct exi_writer exi_writer;

typedef struct exi_strings_config {
    uint32_t size;
    uint32_t hash_bits;     /* 16 or 32 */
    uint32_t chunk_bytes;
    uint32_t _pad;
    uint64_t max_bytes;
    uint64_t max_strings;
} exi_strings_config;

/* Bytes of a digest with its NUL: "sha256:", 64 hex digits, NUL. */
#define EXI_DIGEST_LEN 72

/* ----- reader ----- */

#define EXI_FORMAT_EXI 0
#define EXI_FORMAT_XML 1

#define EXI_SEQ_SINGLE 0
#define EXI_SEQ_STREAMS 1
#define EXI_SEQ_FRAMES 2

#define EXI_RD_XML_DOCTYPE (1 << 3)
#define EXI_RD_XML_PREDEFINED_ONLY (1 << 4)

#define EXI_FEED_FINAL (1 << 0)
#define EXI_FEED_END_FRAME (1 << 1)

typedef struct exi_reader_config {
    uint32_t size;
    uint32_t flags;         /* EXI_RD_* */
    uint32_t sequence;      /* EXI_SEQ_* */
    uint32_t format;        /* EXI_FORMAT_* */
    const exi_options *options;
    exi_grammar_set *grammars;  /* NULL: schema-less */
    const exi_limits *limits;
    exi_strings *strings;   /* NULL: private tables */
    uint32_t input_buffer_size; /* 0 (4096), 64, 256, 1024, 4096, 16384 or 65536 */
    uint32_t _pad;
} exi_reader_config;

/* Non-zero from a handler stops exi_reader_run. Any may be NULL. */
typedef struct exi_handlers {
    uint32_t size;
    int (*start_document)(void *user);
    int (*end_document)(void *user);
    int (*start_element)(void *user, const exi_name *name);
    int (*end_element)(void *user, const exi_name *name);
    int (*attribute)(void *user, const exi_name *name, const exi_value *value);
    int (*characters)(void *user, const exi_value *value);
    int (*namespace_decl)(void *user, const exi_nsdecl *ns);
    int (*comment)(void *user, exi_str text);
    int (*processing_instruction)(void *user, exi_str target, exi_str data);
    int (*doctype)(void *user, exi_str name, exi_str public_id, exi_str system_id,
                   exi_str text);
    int (*entity_reference)(void *user, exi_str name);
    int (*self_contained)(void *user);
} exi_handlers;

/* ----- writer ----- */

#define EXI_HDR_COOKIE (1 << 0)
#define EXI_HDR_NO_OPTIONS (1 << 1)

#define EXI_WR_XML_DECLARATION (1 << 3)
#define EXI_WR_CANONICAL (1 << 4)
#define EXI_WR_UTC_TIME (1 << 5)

/* Output octets; a status other than EXI_OK stops the writer with it. */
typedef exi_status (*exi_write_fn)(void *user, const uint8_t *data, size_t len);

typedef struct exi_writer_config {
    uint32_t size;
    uint32_t flags;         /* EXI_WR_* */
    uint32_t header;        /* EXI_HDR_* */
    uint32_t sequence;      /* EXI_SEQ_* */
    uint32_t format;        /* EXI_FORMAT_* */
    uint32_t _pad;
    const exi_options *options;
    exi_grammar_set *grammars;  /* NULL: schema-less */
    exi_write_fn write;     /* NULL: accumulate, see exi_writer_take */
    void *write_user;
    const exi_limits *limits;
    exi_strings *strings;   /* NULL: private tables */
} exi_writer_config;

/* ----- functions ----- */

EXI_API uint32_t exi_abi_version(void);
EXI_API const char *exi_version_string(void);
EXI_API uint64_t exi_features(void);
EXI_API const char *exi_status_string(exi_status s);

EXI_API void exi_options_init(exi_options *o);
EXI_API exi_status exi_options_validate(const exi_options *o, const char **why);
EXI_API void exi_limits_init(exi_limits *l);

EXI_API exi_strings *exi_strings_new(const exi_strings_config *cfg);
EXI_API void exi_strings_free(exi_strings *s);

EXI_API exi_grammar_set *exi_grammar_set_schemaless(void);
EXI_API exi_grammar_set *exi_grammar_set_retain(exi_grammar_set *g);
EXI_API void exi_grammar_set_release(exi_grammar_set *g);
EXI_API exi_status exi_grammar_set_name_id(const exi_grammar_set *g, exi_str uri,
                                           exi_str local, uint32_t *uri_id,
                                           uint32_t *local_id);
EXI_API exi_status exi_grammar_set_from_exig(const uint8_t *data, size_t len,
                                             exi_grammar_set **out);
EXI_API exi_status exi_grammar_set_to_exig(const exi_grammar_set *g, exi_write_fn write,
                                           void *user);
EXI_API exi_status exi_grammar_set_digest(const exi_grammar_set *g, char *out);

EXI_API exi_reader *exi_reader_new(const exi_reader_config *cfg);
EXI_API void exi_reader_free(exi_reader *r);
EXI_API exi_status exi_reader_feed(exi_reader *r, const void *data, size_t len,
                                   uint32_t flags, size_t *accepted);
EXI_API uint8_t *exi_reader_buffer(exi_reader *r, size_t *len);
EXI_API exi_status exi_reader_commit(exi_reader *r, size_t n);
EXI_API exi_status exi_reader_next(exi_reader *r, exi_event *out);
EXI_API const exi_options *exi_reader_options(const exi_reader *r);
EXI_API exi_status exi_reader_error(const exi_reader *r, const char **msg,
                                    uint64_t *byte_offset, uint64_t *event_index);
EXI_API exi_status exi_reader_run(exi_reader *r, const exi_handlers *h, void *user);

EXI_API exi_writer *exi_writer_new(const exi_writer_config *cfg);
EXI_API void exi_writer_free(exi_writer *w);
EXI_API exi_status exi_writer_event(exi_writer *w, const exi_event *ev);
EXI_API exi_status exi_writer_start_element(exi_writer *w, exi_str uri, exi_str local);
EXI_API exi_status exi_writer_attribute(exi_writer *w, exi_str uri, exi_str local,
                                        exi_str text);
EXI_API exi_status exi_writer_text(exi_writer *w, exi_str text);
EXI_API exi_status exi_writer_end_element(exi_writer *w);
EXI_API exi_status exi_writer_end_document(exi_writer *w);
EXI_API exi_status exi_writer_flush(exi_writer *w, uint32_t *pending_bits);
EXI_API exi_status exi_writer_end_frame(exi_writer *w, const uint8_t **data, size_t *len);
EXI_API exi_status exi_writer_take(exi_writer *w, const uint8_t **data, size_t *len);
EXI_API exi_status exi_writer_error(const exi_writer *w, const char **msg);

#ifdef __cplusplus
}
#endif

#endif /* EXI_H */
