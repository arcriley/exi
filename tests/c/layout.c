/* Prints the C sizes and offsets of libexi's structs, for tests/c_dom.rs to compare
 * with the Rust layout. */
#include <stdio.h>
#include <stddef.h>
#include "exi_dom.h"
#define F(t, f) printf(#t "." #f " %zu\n", offsetof(t, f))
int main(void) {
    printf("exiNode %zu\nexiAttr %zu\nexiNs %zu\nexiDtd %zu\nexiDoc %zu\n", sizeof(exiNode), sizeof(exiAttr), sizeof(exiNs), sizeof(exiDtd), sizeof(exiDoc));
    F(exiNode, content); F(exiNode, nsDef); F(exiNode, line); F(exiNode, uriId); F(exiNode, localId);
    F(exiAttr, ns); F(exiAttr, atype); F(exiAttr, uriId);
    F(exiNs, href); F(exiNs, context); F(exiNs, uriId);
    F(exiDtd, ExternalID); F(exiDtd, pentities);
    F(exiDoc, intSubset); F(exiDoc, URL); F(exiDoc, properties);
    return 0;
}
