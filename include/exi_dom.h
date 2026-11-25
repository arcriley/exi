/* exi_dom.h: libexi layer 2, the document tree (features "capi" and "dom").
 *
 * libxml2's tree API (tree.h, parser.h) under exi names. The structs have libxml2's
 * fields in libxml2's order, so code that walks node->children, cur->next, cur->name,
 * cur->type and node->properties ports unchanged; exi_libxml_compat.h maps libxml2's
 * names onto these.
 *
 * Kept in step with src/capi/dom.rs and src/dom/raw.rs; tests/c_dom.rs checks both the
 * layouts and that every export is declared here.
 *
 * Where libexi differs from libxml2:
 *  - Each element and attribute carries uriId and localId: its name as integers in the
 *    document's global string tables. Documents of one thread share tables unless made
 *    with exiNewDocWithStrings, so names compare as integers across them
 *    (exiNameLookup gives the ids of a name).
 *  - Nodes made without a document (exiNewNode, exiNewText, exiNewComment, exiNewPI,
 *    exiNewProp(NULL, ...)) live in a holding document of the thread; adding one to a
 *    document moves it there and keeps its pointer.
 *  - An unlinked element's ns is NULL until it is placed again, since its declaration
 *    may have gone with its old parent. exiFreeNode unlinks before freeing.
 *  - Text from schema-informed EXI may be typed: content is NULL and
 *    exiNodeGetContent or exiNodeGetValue gives the value.
 *  - exiNewChild's content is text (as xmlNewTextChild): no entity references.
 *  - UTF-8 only: encoding arguments are ignored. format arguments are ignored.
 *  - exiCopyNode with extended 0 copies as with 2.
 *  - Input and output also go through layer 1: exiDocReadFrom (any exi_reader: EXI or
 *    XML text), exiDocWriteTo and exiNodeWriteTo (any exi_writer).
 *
 * Strings returned are malloc'd: free them with exiFree.
 */
#ifndef EXI_DOM_H
#define EXI_DOM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef EXI_DOM_API
#define EXI_DOM_API
#endif

/* Layer 1 types used here (declared by exi.h). */
#ifndef EXI_H
typedef int32_t exi_status;
typedef struct exi_strings exi_strings;
typedef struct exi_reader exi_reader;
typedef struct exi_writer exi_writer;
typedef struct exi_value exi_value;
#endif

typedef unsigned char exiChar;

/* The type field: libxml2's numbers. */
typedef int exiElementType;
#define EXI_ELEMENT_NODE 1
#define EXI_ATTRIBUTE_NODE 2
#define EXI_TEXT_NODE 3
#define EXI_CDATA_SECTION_NODE 4
#define EXI_ENTITY_REF_NODE 5
#define EXI_ENTITY_NODE 6
#define EXI_PI_NODE 7
#define EXI_COMMENT_NODE 8
#define EXI_DOCUMENT_NODE 9
#define EXI_DOCUMENT_TYPE_NODE 10
#define EXI_DOCUMENT_FRAG_NODE 11
#define EXI_NOTATION_NODE 12
#define EXI_HTML_DOCUMENT_NODE 13
#define EXI_DTD_NODE 14
#define EXI_NAMESPACE_DECL 18
typedef exiElementType exiNsType;
#define EXI_LOCAL_NAMESPACE EXI_NAMESPACE_DECL

/* Parser options (xmlParserOption). Only NOBLANKS has an effect; NODICT is accepted
 * because libexi documents never share a libxml2-style dictionary. */
#define EXI_PARSE_NOBLANKS (1 << 8)
#define EXI_PARSE_NODICT (1 << 12)

typedef struct _exiNs exiNs;
typedef exiNs *exiNsPtr;
typedef struct _exiNode exiNode;
typedef exiNode *exiNodePtr;
typedef struct _exiAttr exiAttr;
typedef exiAttr *exiAttrPtr;
typedef struct _exiDtd exiDtd;
typedef exiDtd *exiDtdPtr;
typedef struct _exiDoc exiDoc;
typedef exiDoc *exiDocPtr;

/* No-id marker for uriId and localId. */
#define EXI_DOM_NO_ID UINT32_MAX

struct _exiNs {
    struct _exiNs *next;        /* next declaration on the same element */
    exiNsType type;
    const exiChar *href;
    const exiChar *prefix;      /* NULL for the default namespace */
    void *_private;
    struct _exiDoc *context;
    uint32_t uriId;             /* libexi: href's global id */
    uint32_t _pad;
};

struct _exiNode {
    void *_private;
    exiElementType type;
    const exiChar *name;        /* local name; "text", "comment"; PI target */
    struct _exiNode *children;
    struct _exiNode *last;
    struct _exiNode *parent;
    struct _exiNode *next;
    struct _exiNode *prev;
    struct _exiDoc *doc;
    exiNs *ns;
    exiChar *content;           /* text, comment and PI data; NULL for typed values */
    struct _exiAttr *properties;
    exiNs *nsDef;
    void *psvi;
    unsigned short line;
    unsigned short extra;
    uint32_t uriId;             /* libexi: the name's global ids */
    uint32_t localId;
};

struct _exiAttr {
    void *_private;
    exiElementType type;
    const exiChar *name;
    struct _exiNode *children;  /* the value, as text nodes */
    struct _exiNode *last;
    struct _exiNode *parent;
    struct _exiAttr *next;
    struct _exiAttr *prev;
    struct _exiDoc *doc;
    exiNs *ns;
    int atype;
    void *psvi;
    void *id;
    uint32_t uriId;
    uint32_t localId;
};

struct _exiDtd {
    void *_private;
    exiElementType type;
    const exiChar *name;
    struct _exiNode *children;
    struct _exiNode *last;
    struct _exiDoc *parent;
    struct _exiNode *next;
    struct _exiNode *prev;
    struct _exiDoc *doc;
    void *notations;
    void *elements;
    void *attributes;
    void *entities;
    const exiChar *ExternalID;
    const exiChar *SystemID;
    void *pentities;
};

struct _exiDoc {
    void *_private;
    exiElementType type;
    char *name;
    struct _exiNode *children;
    struct _exiNode *last;
    struct _exiNode *parent;
    struct _exiNode *next;
    struct _exiNode *prev;
    struct _exiDoc *doc;
    int compression;
    int standalone;
    struct _exiDtd *intSubset;
    struct _exiDtd *extSubset;
    struct _exiNs *oldNs;
    const exiChar *version;
    const exiChar *encoding;
    void *ids;
    void *refs;
    const exiChar *URL;
    int charset;
    void *dict;
    void *psvi;
    int parseFlags;
    int properties;
};

/* `xmlNewDoc`. The document uses its thread's string tables. */
EXI_DOM_API exiDocPtr exiNewDoc(const exiChar *version);

/* A new document whose names go in `strings` (an `exi_strings` that outlives it), so its name ids match readers and writers using the same tables. */
EXI_DOM_API exiDocPtr exiNewDocWithStrings(const exiChar *version, exi_strings *strings);

/* `xmlFreeDoc`: the document and every node in it, linked or not. */
EXI_DOM_API void exiFreeDoc(exiDocPtr d);

/* `xmlDocGetRootElement`. */
EXI_DOM_API exiNodePtr exiDocGetRootElement(const exiDoc *d);

/* `xmlDocSetRootElement`: returns the old root, unlinked, or NULL. */
EXI_DOM_API exiNodePtr exiDocSetRootElement(exiDocPtr d, exiNodePtr root);

/* `xmlCopyDoc`: with `recursive` 0, the document without its children. */
EXI_DOM_API exiDocPtr exiCopyDoc(exiDocPtr d, int recursive);

/* `xmlNewDocNode`: an element in `ns` (NULL: no namespace), with `content` as text. */
EXI_DOM_API exiNodePtr exiNewDocNode(exiDocPtr d, exiNsPtr ns, const exiChar *name, const exiChar *content);

/* `xmlNewNode`: an element with no document yet (see the notes at the top of this header). */
EXI_DOM_API exiNodePtr exiNewNode(exiNsPtr ns, const exiChar *name);

/* `xmlNewChild` / `xmlNewTextChild`: a new last child of `parent`. With `ns` NULL the child takes `parent`'s namespace, as libxml2 does. */
EXI_DOM_API exiNodePtr exiNewChild(exiNodePtr parent, exiNsPtr ns, const exiChar *name, const exiChar *content);

/* `xmlNewTextChild`: the same as `exiNewChild` here. */
EXI_DOM_API exiNodePtr exiNewTextChild(exiNodePtr parent, exiNsPtr ns, const exiChar *name, const exiChar *content);

/* `xmlNewDocText`. */
EXI_DOM_API exiNodePtr exiNewDocText(const exiDoc *d, const exiChar *content);

/* `xmlNewDocTextLen`. */
EXI_DOM_API exiNodePtr exiNewDocTextLen(exiDocPtr d, const exiChar *content, int len);

/* `xmlNewText`: a text node with no document yet. */
EXI_DOM_API exiNodePtr exiNewText(const exiChar *content);

/* `xmlNewTextLen`. */
EXI_DOM_API exiNodePtr exiNewTextLen(const exiChar *content, int len);

/* `xmlNewDocComment`. */
EXI_DOM_API exiNodePtr exiNewDocComment(exiDocPtr d, const exiChar *content);

/* `xmlNewComment`. */
EXI_DOM_API exiNodePtr exiNewComment(const exiChar *content);

/* `xmlNewDocPI`. */
EXI_DOM_API exiNodePtr exiNewDocPI(exiDocPtr d, const exiChar *name, const exiChar *content);

/* `xmlNewPI`. */
EXI_DOM_API exiNodePtr exiNewPI(const exiChar *name, const exiChar *content);

/* `xmlNewReference`: `&name;`, or `name` given as `&name;`. */
EXI_DOM_API exiNodePtr exiNewReference(const exiDoc *d, const exiChar *name);

/* `xmlAddChild`: appends `cur` to `parent`. Text is merged into a text `parent` or a text last child, and `cur` freed, as libxml2 does; the node that holds the text is returned. An attribute goes on `parent`'s attributes, replacing one of the same name. */
EXI_DOM_API exiNodePtr exiAddChild(exiNodePtr parent, exiNodePtr cur);

/* `xmlAddChildList`: `cur` and its following siblings. Returns the last added. */
EXI_DOM_API exiNodePtr exiAddChildList(exiNodePtr parent, exiNodePtr cur);

/* `xmlAddNextSibling`. */
EXI_DOM_API exiNodePtr exiAddNextSibling(exiNodePtr cur, exiNodePtr elem);

/* `xmlAddPrevSibling`. */
EXI_DOM_API exiNodePtr exiAddPrevSibling(exiNodePtr cur, exiNodePtr elem);

/* `xmlAddSibling`: after the last sibling of `cur`. */
EXI_DOM_API exiNodePtr exiAddSibling(exiNodePtr cur, exiNodePtr elem);

/* `xmlUnlinkNode`. The node stays its document's until freed. */
EXI_DOM_API void exiUnlinkNode(exiNodePtr cur);

/* `xmlFreeNode`: frees `cur` and everything inside it, unlinking it first. A document is freed as `exiFreeDoc`. */
EXI_DOM_API void exiFreeNode(exiNodePtr cur);

/* `xmlFreeNodeList`: `cur` and its following siblings. */
EXI_DOM_API void exiFreeNodeList(exiNodePtr cur);

/* `xmlReplaceNode`: puts `cur` where `old` is; returns `old`, unlinked. With `cur` NULL, unlinks `old`. */
EXI_DOM_API exiNodePtr exiReplaceNode(exiNodePtr old, exiNodePtr cur);

/* `xmlCopyNode`: `extended` 1 copies everything inside; 0 and 2 copy the node with its attributes and namespace declarations. */
EXI_DOM_API exiNodePtr exiCopyNode(exiNodePtr n, int extended);

/* `xmlDocCopyNode`: a copy in document `d`. */
EXI_DOM_API exiNodePtr exiDocCopyNode(exiNodePtr n, exiDocPtr d, int extended);

/* `xmlNodeGetContent`: malloc'd text, typed values written as text. */
EXI_DOM_API exiChar *exiNodeGetContent(const exiNode *n);

/* `xmlNodeSetContent`: 0, or -1. */
EXI_DOM_API int exiNodeSetContent(exiNodePtr n, const exiChar *content);

/* `xmlNodeSetContentLen`. */
EXI_DOM_API int exiNodeSetContentLen(exiNodePtr n, const exiChar *content, int len);

/* `xmlNodeAddContent`. */
EXI_DOM_API int exiNodeAddContent(exiNodePtr n, const exiChar *content);

/* `xmlNodeAddContentLen`. */
EXI_DOM_API int exiNodeAddContentLen(exiNodePtr n, const exiChar *content, int len);

/* `xmlNodeListGetString`: the text of `list` and its following siblings; entity references as `&name;` unless `inLine`. NULL for an empty result. */
EXI_DOM_API exiChar *exiNodeListGetString(exiDocPtr d, const exiNode *list, int inLine);

/* `xmlNodeIsText`. */
EXI_DOM_API int exiNodeIsText(const exiNode *n);

/* `xmlIsBlankNode`. */
EXI_DOM_API int exiIsBlankNode(const exiNode *n);

/* `xmlNodeSetName`. */
EXI_DOM_API void exiNodeSetName(exiNodePtr n, const exiChar *name);

/* `xmlGetLineNo`: the line of a node read from XML text, or -1. */
EXI_DOM_API long exiGetLineNo(const exiNode *n);

/* `xmlGetNodePath`: malloc'd. */
EXI_DOM_API exiChar *exiGetNodePath(const exiNode *n);

/* A text node's or attribute's value, typed as it came: `out` is valid until the next `exiNodeGetValue` on the same document or a change to the node. `EXI_E_ARG` for other nodes. */
EXI_DOM_API exi_status exiNodeGetValue(const exiNode *n, exi_value *out);

/* Sets a text node's, attribute's or element's content to a typed value. */
EXI_DOM_API exi_status exiNodeSetValue(exiNodePtr n, const exi_value *v);

/* `xmlNewProp`: an attribute on `node` (one of the same name gets the new value), or with `node` NULL a detached one. */
EXI_DOM_API exiAttrPtr exiNewProp(exiNodePtr n, const exiChar *name, const exiChar *value);

/* `xmlNewNsProp`. */
EXI_DOM_API exiAttrPtr exiNewNsProp(exiNodePtr n, exiNsPtr ns, const exiChar *name, const exiChar *value);

/* `xmlSetProp`: `name` may be `prefix:local`, resolved at `node`. */
EXI_DOM_API exiAttrPtr exiSetProp(exiNodePtr n, const exiChar *name, const exiChar *value);

/* `xmlSetNsProp`. */
EXI_DOM_API exiAttrPtr exiSetNsProp(exiNodePtr n, exiNsPtr ns, const exiChar *name, const exiChar *value);

/* `xmlHasProp`: the attribute named `name` in any namespace. */
EXI_DOM_API exiAttrPtr exiHasProp(const exiNode *n, const exiChar *name);

/* `xmlHasNsProp`: `nameSpace` NULL for no namespace. */
EXI_DOM_API exiAttrPtr exiHasNsProp(const exiNode *n, const exiChar *name, const exiChar *nameSpace);

/* `xmlGetProp`: malloc'd, or NULL. Any namespace. */
EXI_DOM_API exiChar *exiGetProp(const exiNode *n, const exiChar *name);

/* `xmlGetNoNsProp`. */
EXI_DOM_API exiChar *exiGetNoNsProp(const exiNode *n, const exiChar *name);

/* `xmlGetNsProp`. */
EXI_DOM_API exiChar *exiGetNsProp(const exiNode *n, const exiChar *name, const exiChar *nameSpace);

/* `xmlRemoveProp`: unlinks and frees; 0, or -1. */
EXI_DOM_API int exiRemoveProp(exiAttrPtr a);

/* `xmlFreeProp`. */
EXI_DOM_API void exiFreeProp(exiAttrPtr a);

/* `xmlUnsetProp`: the attribute in no namespace. 0, or -1 if there was none. */
EXI_DOM_API int exiUnsetProp(exiNodePtr n, const exiChar *name);

/* `xmlUnsetNsProp`. */
EXI_DOM_API int exiUnsetNsProp(exiNodePtr n, exiNsPtr ns, const exiChar *name);

/* `xmlNewNs`: declares `prefix` (NULL: the default namespace) for `href` on `node`. */
EXI_DOM_API exiNsPtr exiNewNs(exiNodePtr n, const exiChar *href, const exiChar *prefix);

/* `xmlSearchNs`: the declaration `prefix` (NULL: default) is bound to at `node`. */
EXI_DOM_API exiNsPtr exiSearchNs(exiDocPtr d, exiNodePtr n, const exiChar *prefix);

/* `xmlSearchNsByHref`. */
EXI_DOM_API exiNsPtr exiSearchNsByHref(exiDocPtr d, exiNodePtr n, const exiChar *href);

/* `xmlSetNs`: puts an element or attribute in `ns`'s namespace (NULL: none). */
EXI_DOM_API void exiSetNs(exiNodePtr n, exiNsPtr ns);

/* `xmlGetNsList`: the declarations in scope at `node`, nearest first, one per prefix, as a malloc'd NULL-terminated array (free with `exiFree`); NULL if none. */
EXI_DOM_API exiNsPtr *exiGetNsList(const exiDoc *d, const exiNode *n);

/* `xmlFirstElementChild`. */
EXI_DOM_API exiNodePtr exiFirstElementChild(exiNodePtr n);

/* `xmlLastElementChild`. */
EXI_DOM_API exiNodePtr exiLastElementChild(exiNodePtr n);

/* `xmlNextElementSibling`. */
EXI_DOM_API exiNodePtr exiNextElementSibling(exiNodePtr n);

/* `xmlPreviousElementSibling`. */
EXI_DOM_API exiNodePtr exiPreviousElementSibling(exiNodePtr n);

/* `xmlChildElementCount`. */
EXI_DOM_API unsigned long exiChildElementCount(exiNodePtr n);

/* The global ids of a name, if the document's tables hold it: compare with a node's `uriId` and `localId`. 1 if found, else 0. */
EXI_DOM_API int exiNameLookup(const exiDoc *d, const exiChar *uri, const exiChar *local, uint32_t *uriId, uint32_t *localId);

/* `xmlReadMemory`: XML text. `options`: `EXI_PARSE_NOBLANKS`; others are ignored. */
EXI_DOM_API exiDocPtr exiReadMemory(const char *buffer, int size, const char *URL, const char *encoding, int options);

/* `xmlReadDoc`: a NUL-terminated document. */
EXI_DOM_API exiDocPtr exiReadDoc(const exiChar *cur, const char *URL, const char *encoding, int options);

/* `xmlReadFile`. */
EXI_DOM_API exiDocPtr exiReadFile(const char *filename, const char *encoding, int options);

/* `xmlParseMemory`. */
EXI_DOM_API exiDocPtr exiParseMemory(const char *buffer, int size);

/* `xmlParseDoc`. */
EXI_DOM_API exiDocPtr exiParseDoc(const exiChar *cur);

/* `xmlParseFile`. */
EXI_DOM_API exiDocPtr exiParseFile(const char *filename);

/* A document from every event of a layer 1 reader: EXI, or XML text with `EXI_FORMAT_XML`. The reader must have all its input (`EXI_FEED_FINAL`). Names go in `strings` (NULL: the thread's tables); give the reader's own `exi_strings` for name ids that match the reader's. NULL on error: `exi_reader_error` tells why. */
EXI_DOM_API exiDocPtr exiDocReadFrom(exi_reader *r, exi_strings *strings);

/* Writes the whole document to a layer 1 writer (EXI, or XML text with `EXI_FORMAT_XML`), ending it. Flush the writer after. */
EXI_DOM_API exi_status exiDocWriteTo(const exiDoc *d, exi_writer *w);

/* Writes one element and everything in it, with the declarations it needs: a stanza for an open stream. */
EXI_DOM_API exi_status exiNodeWriteTo(const exiNode *n, exi_writer *w);

/* `xmlDocDumpMemory`: XML text with a declaration, malloc'd (free with `exiFree`). */
EXI_DOM_API void exiDocDumpMemory(const exiDoc *d, exiChar **mem, int *size);

/* `xmlDocDumpFormatMemory`: `format` is ignored. */
EXI_DOM_API void exiDocDumpFormatMemory(const exiDoc *d, exiChar **mem, int *size, int format);

/* One element as XML text, malloc'd: the subtree with the declarations it needs. */
EXI_DOM_API void exiNodeDumpMemory(const exiNode *n, exiChar **mem, int *size);

/* `xmlSaveFile`: the octets written, or -1. */
EXI_DOM_API int exiSaveFile(const char *filename, const exiDoc *d);

/* `xmlSaveFormatFile`: `format` is ignored. */
EXI_DOM_API int exiSaveFormatFile(const char *filename, const exiDoc *d, int format);

/* `xmlFree`: frees a string or array this API returned. */
EXI_DOM_API void exiFree(void *p);

#ifdef __cplusplus
}
#endif

#endif /* EXI_DOM_H */
