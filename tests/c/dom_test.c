/* A libxml2 program. Built against libexi with exi_libxml_compat.h, and against
 * libxml2 with -DUSE_LIBXML2; tests/c_dom.rs runs both and compares what they print. */
#include <stdio.h>
#include <stdlib.h>
#ifdef USE_LIBXML2
#include "libxml2_decls.h"
#else
#include "exi_libxml_compat.h"
#endif

static void put(const xmlChar *s) {
    if (!s) { printf("(null)"); return; }
    for (; *s; s++) {
        if (*s == '\n') printf("\\n");
        else putchar(*s);
    }
}

static void walk(xmlNodePtr n, int depth) {
    for (xmlNodePtr cur = n; cur; cur = cur->next) {
        printf("%*s%d ", depth * 2, "", (int)cur->type);
        put(cur->name);
        if (cur->type == XML_ELEMENT_NODE) {
            if (cur->ns) { printf(" {"); put(cur->ns->href); printf("}"); if (cur->ns->prefix) put(cur->ns->prefix); }
            printf(" line=%ld", xmlGetLineNo(cur));
            for (xmlNsPtr d = cur->nsDef; d; d = d->next) { printf(" xmlns:"); if (d->prefix) put(d->prefix); printf("="); put(d->href); }
            for (xmlAttrPtr a = cur->properties; a; a = a->next) {
                printf(" @");
                if (a->ns && a->ns->prefix) { put(a->ns->prefix); printf(":"); }
                put(a->name);
                printf("=");
                put(a->children ? a->children->content : NULL);
                if (a->parent != cur) printf(" BAD-PARENT");
            }
            printf("\n");
            walk(cur->children, depth + 1);
        } else if (cur->type == XML_DTD_NODE) {
            printf(" system="); put(((xmlDtdPtr)cur)->SystemID); printf("\n");
        } else {
            printf(" ["); put(cur->content); printf("]\n");
        }
        if (cur->next && cur->next->prev != cur) printf("BAD-PREV\n");
        if (cur->parent && cur->next == NULL && cur->parent->last != cur) printf("BAD-LAST\n");
    }
}

static void dump(const char *what, xmlDocPtr doc) {
    xmlChar *mem = NULL;
    int size = 0;
    xmlDocDumpMemory(doc, &mem, &size);
    /* Skip the XML declaration line: libxml2 and libexi write it differently. */
    const char *body = strchr((const char *)mem, '\n');
    printf("%s: %s", what, body ? body + 1 : (const char *)mem);
    xmlFree(mem);
}

static void str(const char *what, xmlChar *s) {
    printf("%s: ", what);
    put(s);
    printf("\n");
    xmlFree(s);
}

static const char *doc_text =
    "<?xml version=\"1.0\"?>\n"
    "<!DOCTYPE svg SYSTEM \"svg.dtd\">\n"
    "<!-- drawing -->\n"
    "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"10\">\n"
    "  <title xml:lang=\"en\">Logo &amp; mark</title>\n"
    "  <g id=\"a\" fill=\"red\"><rect x=\"1\"/><circle r=\"2\"/></g>\n"
    "  <use xlink:href=\"#a\"/>\n"
    "  <?app keep this?>\n"
    "  <desc>one <b xmlns=\"urn:b\">two</b> three</desc>\n"
    "</svg>\n";

int main(void) {
    /* 1. Parse and walk the structs. */
    /* NODICT: libxml2 needs it to move nodes between documents safely (section 4). */
    xmlDocPtr doc = xmlReadMemory(doc_text, (int)strlen(doc_text), "test.xml", NULL, XML_PARSE_NODICT);
    if (!doc) { printf("parse failed\n"); return 1; }
    printf("URL: %s\n", (const char *)doc->URL);
    walk(doc->children, 0);
    xmlNodePtr svg = xmlDocGetRootElement(doc);
    if (svg->parent != (xmlNodePtr)doc || svg->doc != doc) printf("BAD-ROOT\n");

    /* 2. Reading. */
    xmlNodePtr title = xmlFirstElementChild(svg);
    xmlNodePtr g = xmlNextElementSibling(title);
    xmlNodePtr use = xmlNextElementSibling(g);
    xmlNodePtr desc = xmlLastElementChild(svg);
    printf("children: %lu, prev of use: %s\n", xmlChildElementCount(svg), (const char *)xmlPreviousElementSibling(use)->name);
    str("width", xmlGetProp(svg, BAD_CAST "width"));
    str("href", xmlGetNsProp(use, BAD_CAST "href", BAD_CAST "http://www.w3.org/1999/xlink"));
    str("href any ns", xmlGetProp(use, BAD_CAST "href"));
    str("missing", xmlGetProp(svg, BAD_CAST "height"));
    str("title", xmlNodeGetContent(title));
    str("desc", xmlNodeGetContent(desc));
    str("path g", xmlGetNodePath(g));
    str("path rect", xmlGetNodePath(g->children));
    str("path text", xmlGetNodePath(desc->children));
    printf("blank first child: %d\n", xmlIsBlankNode(svg->children));
    xmlNsPtr xl = xmlSearchNs(doc, use, BAD_CAST "xlink");
    printf("xlink: %s\n", xl ? (const char *)xl->href : "(none)");
    xmlNsPtr def = xmlSearchNsByHref(doc, g, BAD_CAST "http://www.w3.org/2000/svg");
    printf("svg prefix: %s\n", def ? (def->prefix ? (const char *)def->prefix : "(default)") : "(none)");

    /* 3. Editing. */
    xmlNodePtr rect = g->children;
    xmlUnlinkNode(rect);
    xmlAddNextSibling(g->children, rect);              /* rect after circle */
    xmlSetProp(g, BAD_CAST "fill", BAD_CAST "blue");
    xmlSetProp(g, BAD_CAST "stroke", BAD_CAST "black");
    xmlUnsetProp(g, BAD_CAST "id");
    xmlNodeSetName(rect, BAD_CAST "square");
    xmlNodeSetContent(title, BAD_CAST "New title");
    xmlNodeAddContent(title, BAD_CAST " (2)");
    xmlNodePtr copy = xmlCopyNode(g, 1);
    xmlAddChild(svg, copy);
    xmlNodePtr text = xmlNewText(BAD_CAST "after");
    xmlAddPrevSibling(use, text);
    xmlNodePtr old = xmlReplaceNode(use, xmlNewDocNode(doc, svg->ns, BAD_CAST "line", NULL));
    xmlFreeNode(old);
    xmlNodePtr gone = desc;
    xmlUnlinkNode(gone);
    xmlFreeNode(gone);
    dump("edited", doc);

    /* 4. Building a document, as libxml2 tutorials do. */
    xmlDocPtr out = xmlNewDoc(BAD_CAST "1.0");
    xmlNodePtr root = xmlNewNode(NULL, BAD_CAST "root");
    xmlDocSetRootElement(out, root);
    xmlNsPtr ns = xmlNewNs(root, BAD_CAST "urn:x", BAD_CAST "x");
    xmlNodePtr item = xmlNewChild(root, ns, BAD_CAST "item", BAD_CAST "text");
    xmlSetProp(item, BAD_CAST "id", BAD_CAST "1");
    xmlNewNsProp(item, ns, BAD_CAST "k", BAD_CAST "v");
    xmlNodePtr inherit = xmlNewChild(item, NULL, BAD_CAST "inner", NULL);
    printf("inner ns: %s\n", inherit->ns ? (const char *)inherit->ns->href : "(none)");
    xmlAddChild(root, xmlNewText(BAD_CAST "tail"));
    xmlNodePtr merged = xmlAddChild(root, xmlNewText(BAD_CAST "+more"));
    printf("merged: %s, last is merged: %d\n", (const char *)merged->content, root->last == merged);
    xmlAddChild(root, xmlNewComment(BAD_CAST " note "));
    xmlAddChild(root, xmlNewDocPI(out, BAD_CAST "pi", BAD_CAST "data"));
    /* A node of another document, moved in. */
    xmlNodePtr moved = xmlFirstElementChild(xmlDocGetRootElement(doc));
    xmlUnlinkNode(moved);
    xmlAddChild(root, moved);
    printf("moved doc: %d\n", moved->doc == out);
    /* "!": a line where libexi differs on purpose. libxml2 leaves the moved node's
     * namespace declaration in the old document and writes the node without it. */
    dump("!built", out);
    dump("source", doc);

    xmlDocPtr dup = xmlCopyDoc(out, 1);
    dump("!copied", dup);
    xmlFreeDoc(dup);
    xmlFreeDoc(out);
    xmlFreeDoc(doc);
    printf("done\n");
    return 0;
}
