/* The parts of libxml2 2.9's tree.h and parser.h that dom_test.c uses, for building the
 * test against the real libxml2 where only its shared library is installed. The
 * structs are libxml2's. Test-only: not part of libexi. */
#include <string.h>
typedef unsigned char xmlChar;
typedef enum { XML_ELEMENT_NODE = 1, XML_ATTRIBUTE_NODE, XML_TEXT_NODE, XML_CDATA_SECTION_NODE, XML_ENTITY_REF_NODE,
               XML_ENTITY_NODE, XML_PI_NODE, XML_COMMENT_NODE, XML_DOCUMENT_NODE, XML_DOCUMENT_TYPE_NODE,
               XML_DOCUMENT_FRAG_NODE, XML_NOTATION_NODE, XML_HTML_DOCUMENT_NODE, XML_DTD_NODE } xmlElementType;
typedef struct _xmlNs { struct _xmlNs *next; xmlElementType type; const xmlChar *href; const xmlChar *prefix; void *_private; struct _xmlDoc *context; } xmlNs, *xmlNsPtr;
typedef struct _xmlNode { void *_private; xmlElementType type; const xmlChar *name; struct _xmlNode *children, *last, *parent, *next, *prev; struct _xmlDoc *doc;
    xmlNs *ns; xmlChar *content; struct _xmlAttr *properties; xmlNs *nsDef; void *psvi; unsigned short line, extra; } xmlNode, *xmlNodePtr;
typedef struct _xmlAttr { void *_private; xmlElementType type; const xmlChar *name; struct _xmlNode *children, *last, *parent; struct _xmlAttr *next, *prev; struct _xmlDoc *doc;
    xmlNs *ns; int atype; void *psvi; void *id; } xmlAttr, *xmlAttrPtr;
typedef struct _xmlDtd { void *_private; xmlElementType type; const xmlChar *name; struct _xmlNode *children, *last; struct _xmlDoc *parent; struct _xmlNode *next, *prev; struct _xmlDoc *doc;
    void *notations, *elements, *attributes, *entities; const xmlChar *ExternalID, *SystemID; void *pentities; } xmlDtd, *xmlDtdPtr;
typedef struct _xmlDoc { void *_private; xmlElementType type; char *name; struct _xmlNode *children, *last, *parent, *next, *prev; struct _xmlDoc *doc;
    int compression, standalone; struct _xmlDtd *intSubset, *extSubset; struct _xmlNs *oldNs; const xmlChar *version, *encoding; void *ids, *refs;
    const xmlChar *URL; int charset; void *dict; void *psvi; int parseFlags, properties; } xmlDoc, *xmlDocPtr;
#define BAD_CAST (xmlChar *)
#define XML_PARSE_NODICT (1 << 12)
extern void (*xmlFree)(void *);
xmlDocPtr xmlReadMemory(const char *, int, const char *, const char *, int);
xmlDocPtr xmlNewDoc(const xmlChar *);
void xmlFreeDoc(xmlDocPtr);
xmlNodePtr xmlDocGetRootElement(const xmlDoc *);
xmlNodePtr xmlDocSetRootElement(xmlDocPtr, xmlNodePtr);
xmlNodePtr xmlNewNode(xmlNsPtr, const xmlChar *);
xmlNodePtr xmlNewDocNode(xmlDocPtr, xmlNsPtr, const xmlChar *, const xmlChar *);
xmlNodePtr xmlNewChild(xmlNodePtr, xmlNsPtr, const xmlChar *, const xmlChar *);
xmlNodePtr xmlNewText(const xmlChar *);
xmlNodePtr xmlNewComment(const xmlChar *);
xmlNodePtr xmlNewDocPI(xmlDocPtr, const xmlChar *, const xmlChar *);
xmlNodePtr xmlAddChild(xmlNodePtr, xmlNodePtr);
xmlNodePtr xmlAddPrevSibling(xmlNodePtr, xmlNodePtr);
xmlNodePtr xmlAddNextSibling(xmlNodePtr, xmlNodePtr);
xmlNodePtr xmlReplaceNode(xmlNodePtr, xmlNodePtr);
xmlNodePtr xmlCopyNode(xmlNodePtr, int);
void xmlUnlinkNode(xmlNodePtr);
void xmlFreeNode(xmlNodePtr);
xmlNsPtr xmlNewNs(xmlNodePtr, const xmlChar *, const xmlChar *);
xmlNsPtr xmlSearchNs(xmlDocPtr, xmlNodePtr, const xmlChar *);
xmlNsPtr xmlSearchNsByHref(xmlDocPtr, xmlNodePtr, const xmlChar *);
xmlAttrPtr xmlSetProp(xmlNodePtr, const xmlChar *, const xmlChar *);
xmlAttrPtr xmlNewNsProp(xmlNodePtr, xmlNsPtr, const xmlChar *, const xmlChar *);
xmlAttrPtr xmlHasProp(const xmlNode *, const xmlChar *);
xmlChar *xmlGetProp(const xmlNode *, const xmlChar *);
xmlChar *xmlGetNsProp(const xmlNode *, const xmlChar *, const xmlChar *);
int xmlUnsetProp(xmlNodePtr, const xmlChar *);
xmlChar *xmlNodeGetContent(const xmlNode *);
void xmlNodeSetContent(xmlNodePtr, const xmlChar *);
void xmlNodeAddContent(xmlNodePtr, const xmlChar *);
void xmlNodeSetName(xmlNodePtr, const xmlChar *);
long xmlGetLineNo(const xmlNode *);
xmlChar *xmlGetNodePath(const xmlNode *);
int xmlIsBlankNode(const xmlNode *);
xmlNodePtr xmlFirstElementChild(xmlNodePtr);
xmlNodePtr xmlLastElementChild(xmlNodePtr);
xmlNodePtr xmlNextElementSibling(xmlNodePtr);
xmlNodePtr xmlPreviousElementSibling(xmlNodePtr);
unsigned long xmlChildElementCount(xmlNodePtr);
void xmlDocDumpMemory(xmlDocPtr, xmlChar **, int *);
xmlDocPtr xmlCopyDoc(xmlDocPtr, int);
