#include "exi.h"
#include <assert.h>
int main(void) {
    assert(exi_abi_version() == ((EXI_ABI_MAJOR << 16) | EXI_ABI_MINOR));
    exi_options opts;
    exi_options_init(&opts);
    opts.coding = EXI_CODING_BYTE_ALIGNED;
    const exi_writer_config wc = {.size = sizeof wc, .options = &opts};
    exi_writer *w = exi_writer_new(&wc);
    assert(w);
    assert(exi_writer_start_element(w, (exi_str){0}, (exi_str){"x", 1}) == EXI_OK);
    assert(exi_writer_end_element(w) == EXI_OK);
    assert(exi_writer_end_document(w) == EXI_OK);
    const uint8_t *bytes;
    size_t len, taken;
    assert(exi_writer_take(w, &bytes, &len) == EXI_OK && len);
    const exi_reader_config rc = {.size = sizeof rc};
    exi_reader *r = exi_reader_new(&rc);
    assert(r);
    assert(exi_reader_feed(r, bytes, len, EXI_FEED_FINAL, &taken) == EXI_OK && taken == len);
    const uint32_t kinds[] = {EXI_EV_SD, EXI_EV_SE, EXI_EV_EE, EXI_EV_ED};
    exi_event ev;
    for (size_t i = 0; i < sizeof kinds / sizeof *kinds; ++i) {
        assert(exi_reader_next(r, &ev) == EXI_OK);
        assert(ev.type == kinds[i]);
    }
    assert(exi_reader_next(r, &ev) == EXI_END);
    exi_reader_free(r);
    exi_writer_free(w);
}
