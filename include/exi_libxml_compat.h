/* exi_libxml_compat.h: libxml2's tree API names for libexi's.
 *
 * Include this instead of <libxml/tree.h> and <libxml/parser.h> to build a libxml2
 * program against libexi. The names are macros, so a program can still link libxml2
 * for what libexi does not do; do not include both headers in one file.
 * See exi_dom.h for where the behaviour differs.
 */
#ifndef EXI_LIBXML_COMPAT_H
#define EXI_LIBXML_COMPAT_H

#include <string.h>
#include "exi_dom.h"

typedef exiChar xmlChar;
typedef exiElementType xmlElementType;
typedef exiNsType xmlNsType;
typedef exiNs xmlNs;
typedef exiNsPtr xmlNsPtr;
typedef exiNode xmlNode;
typedef exiNodePtr xmlNodePtr;
typedef exiAttr xmlAttr;
typedef exiAttrPtr xmlAttrPtr;
typedef exiDtd xmlDtd;
typedef exiDtdPtr xmlDtdPtr;
typedef exiDoc xmlDoc;
typedef exiDocPtr xmlDocPtr;

#define BAD_CAST (xmlChar *)

#define XML_ELEMENT_NODE EXI_ELEMENT_NODE
#define XML_ATTRIBUTE_NODE EXI_ATTRIBUTE_NODE
#define XML_TEXT_NODE EXI_TEXT_NODE
#define XML_CDATA_SECTION_NODE EXI_CDATA_SECTION_NODE
#define XML_ENTITY_REF_NODE EXI_ENTITY_REF_NODE
#define XML_ENTITY_NODE EXI_ENTITY_NODE
#define XML_PI_NODE EXI_PI_NODE
#define XML_COMMENT_NODE EXI_COMMENT_NODE
#define XML_DOCUMENT_NODE EXI_DOCUMENT_NODE
#define XML_DOCUMENT_TYPE_NODE EXI_DOCUMENT_TYPE_NODE
#define XML_DOCUMENT_FRAG_NODE EXI_DOCUMENT_FRAG_NODE
#define XML_NOTATION_NODE EXI_NOTATION_NODE
#define XML_HTML_DOCUMENT_NODE EXI_HTML_DOCUMENT_NODE
#define XML_DTD_NODE EXI_DTD_NODE
#define XML_NAMESPACE_DECL EXI_NAMESPACE_DECL
#define XML_LOCAL_NAMESPACE EXI_LOCAL_NAMESPACE
#define XML_PARSE_NOBLANKS EXI_PARSE_NOBLANKS
#define XML_PARSE_NODICT EXI_PARSE_NODICT

#define xmlFree exiFree
#define xmlStrcmp(a, b) strcmp((const char *)(a), (const char *)(b))
#define xmlStrEqual(a, b) (strcmp((const char *)(a), (const char *)(b)) == 0)
/* No-ops: libexi has no global parser state. */
#define xmlInitParser() ((void)0)
#define xmlCleanupParser() ((void)0)

#define xmlNewDoc exiNewDoc
#define xmlFreeDoc exiFreeDoc
#define xmlDocGetRootElement exiDocGetRootElement
#define xmlDocSetRootElement exiDocSetRootElement
#define xmlCopyDoc exiCopyDoc
#define xmlNewDocNode exiNewDocNode
#define xmlNewNode exiNewNode
#define xmlNewChild exiNewChild
#define xmlNewTextChild exiNewTextChild
#define xmlNewDocText exiNewDocText
#define xmlNewDocTextLen exiNewDocTextLen
#define xmlNewText exiNewText
#define xmlNewTextLen exiNewTextLen
#define xmlNewDocComment exiNewDocComment
#define xmlNewComment exiNewComment
#define xmlNewDocPI exiNewDocPI
#define xmlNewPI exiNewPI
#define xmlNewReference exiNewReference
#define xmlAddChild exiAddChild
#define xmlAddChildList exiAddChildList
#define xmlAddNextSibling exiAddNextSibling
#define xmlAddPrevSibling exiAddPrevSibling
#define xmlAddSibling exiAddSibling
#define xmlUnlinkNode exiUnlinkNode
#define xmlFreeNode exiFreeNode
#define xmlFreeNodeList exiFreeNodeList
#define xmlReplaceNode exiReplaceNode
#define xmlCopyNode exiCopyNode
#define xmlDocCopyNode exiDocCopyNode
#define xmlNodeGetContent exiNodeGetContent
#define xmlNodeSetContent exiNodeSetContent
#define xmlNodeSetContentLen exiNodeSetContentLen
#define xmlNodeAddContent exiNodeAddContent
#define xmlNodeAddContentLen exiNodeAddContentLen
#define xmlNodeListGetString exiNodeListGetString
#define xmlNodeIsText exiNodeIsText
#define xmlIsBlankNode exiIsBlankNode
#define xmlNodeSetName exiNodeSetName
#define xmlGetLineNo exiGetLineNo
#define xmlGetNodePath exiGetNodePath
#define xmlNewProp exiNewProp
#define xmlNewNsProp exiNewNsProp
#define xmlSetProp exiSetProp
#define xmlSetNsProp exiSetNsProp
#define xmlHasProp exiHasProp
#define xmlHasNsProp exiHasNsProp
#define xmlGetProp exiGetProp
#define xmlGetNoNsProp exiGetNoNsProp
#define xmlGetNsProp exiGetNsProp
#define xmlRemoveProp exiRemoveProp
#define xmlFreeProp exiFreeProp
#define xmlUnsetProp exiUnsetProp
#define xmlUnsetNsProp exiUnsetNsProp
#define xmlNewNs exiNewNs
#define xmlSearchNs exiSearchNs
#define xmlSearchNsByHref exiSearchNsByHref
#define xmlSetNs exiSetNs
#define xmlGetNsList exiGetNsList
#define xmlFirstElementChild exiFirstElementChild
#define xmlLastElementChild exiLastElementChild
#define xmlNextElementSibling exiNextElementSibling
#define xmlPreviousElementSibling exiPreviousElementSibling
#define xmlChildElementCount exiChildElementCount
#define xmlReadMemory exiReadMemory
#define xmlReadDoc exiReadDoc
#define xmlReadFile exiReadFile
#define xmlParseMemory exiParseMemory
#define xmlParseDoc exiParseDoc
#define xmlParseFile exiParseFile
#define xmlDocDumpMemory exiDocDumpMemory
#define xmlDocDumpFormatMemory exiDocDumpFormatMemory
#define xmlSaveFile exiSaveFile
#define xmlSaveFormatFile exiSaveFormatFile
#define xmlFree exiFree

#endif /* EXI_LIBXML_COMPAT_H */
