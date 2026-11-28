// benches/common/corpus.rs
//
// Deterministic XML test documents shared by the XML benches
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//! Synthetic SVG and order documents of a chosen size.

pub struct Lcg(u64);

impl Lcg {
    pub fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// Namespaced, attribute-heavy SVG drawing (matches `benches/throughput.rs`).
pub fn svg(target: usize) -> String {
    let mut r = Lcg(1);
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"1024\" height=\"768\" viewBox=\"0 0 1024 768\" version=\"1.1\">\n  <defs>\n",
    );
    let colors = [
        "#ff6a00", "#ee0979", "#7a00ff", "#00c3ff", "#222222", "#ffffff", "#3c9d4e",
    ];
    for i in 0..8 {
        s += &format!(
            "    <linearGradient id=\"g{i}\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\">\n      <stop offset=\"0\" stop-color=\"{}\"/>\n      <stop offset=\"1\" stop-color=\"{}\"/>\n    </linearGradient>\n",
            colors[i % 7],
            colors[(i + 3) % 7]
        );
    }
    s += "  </defs>\n";
    let mut layer = 0;
    while s.len() < target {
        s += &format!(
            "  <g id=\"layer{layer}\" transform=\"translate({},{})\" fill=\"url(#g{})\" stroke=\"{}\" stroke-width=\"{}\">\n",
            r.next(900),
            r.next(700),
            r.next(8),
            colors[r.next(7) as usize],
            1 + r.next(4)
        );
        for _ in 0..20 {
            match r.next(4) {
                0 | 1 => {
                    let mut d = format!("M{} {}", r.next(1000), r.next(800));
                    for _ in 0..(3 + r.next(6)) {
                        d += &format!(
                            " C {}.{} {}.{}, {} {}, {} {}",
                            r.next(1000),
                            r.next(10),
                            r.next(800),
                            r.next(10),
                            r.next(1000),
                            r.next(800),
                            r.next(1000),
                            r.next(800)
                        );
                    }
                    s += &format!("    <path d=\"{d} Z\" opacity=\"0.{}\"/>\n", 1 + r.next(9));
                }
                2 => {
                    s += &format!(
                        "    <circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>\n",
                        r.next(1000),
                        r.next(800),
                        1 + r.next(80),
                        colors[r.next(7) as usize]
                    )
                }
                _ => {
                    s += &format!(
                        "    <use xlink:href=\"#layer{}\" transform=\"scale(0.{})\"/>\n",
                        r.next(layer + 1),
                        1 + r.next(9)
                    )
                }
            }
        }
        s += &format!(
            "    <text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"{}\">Layer {layer}</text>\n  </g>\n",
            r.next(1000),
            r.next(800),
            8 + r.next(30)
        );
        layer += 1;
    }
    s + "</svg>\n"
}

/// Element- and text-heavy orders feed (matches `benches/throughput.rs`).
pub fn orders(target: usize) -> String {
    let mut r = Lcg(2);
    let names = [
        "Ada", "Grace", "Linus", "Barbara", "Ken", "Margaret", "Dennis", "Frances",
    ];
    let status = ["open", "paid", "shipped", "returned"];
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<orders>\n");
    let mut id = 100000;
    while s.len() < target {
        s += &format!(
            "  <order id=\"{id}\" paid=\"{}\">\n    <customer>{}</customer>\n    <date>2026-{:02}-{:02}</date>\n",
            r.next(2) == 0,
            names[r.next(8) as usize],
            1 + r.next(12),
            1 + r.next(28)
        );
        for _ in 0..(1 + r.next(5)) {
            s += &format!(
                "    <line sku=\"{}-{}\" qty=\"{}\" price=\"{}.{:02}\"/>\n",
                (b'A' + r.next(26) as u8) as char,
                r.next(100),
                1 + r.next(20),
                r.next(500),
                r.next(100)
            );
        }
        if r.next(4) == 0 {
            s += "    <note>leave at the door</note>\n";
        }
        s += &format!(
            "    <status>{}</status>\n  </order>\n",
            status[r.next(4) as usize]
        );
        id += 1;
    }
    s + "</orders>\n"
}
