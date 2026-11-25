// Generates the EXIficient reference fixtures in this directory.
//
// Run from the repository root (Java 11 or later; no compile step):
//
//     fixtures/fetch-exificient.sh            # once: downloads the jars to fixtures/jars
//     java -cp 'fixtures/jars/*' fixtures/MakeFixtures.java fixtures/manifest.tsv
//
// Each line of the manifest is: name, options, input, schema (or -), separated by tabs.
// The program reads `fixtures/input/<input>` and writes `fixtures/exificient/<name>.exi`.
// Options are comma separated:
//
//   bit | byte | pre | compression     coding (default bit)
//   strict, fragment, sc                 strict, fragment, selfContained
//   prefixes, comments, pis, dtd, lexical   the Preserve options
//   options                              write the options document in the header
//   cookie                               write the $EXI cookie
//   canonical, utc                       Canonical EXI, and its utcTime option
//   block=N, vml=N, vpc=N                blockSize, valueMaxLength, valuePartitionCapacity
//   decode                               the input is EXI: decode it to <name>.xml instead
//   grammars                             write EXIficient's grammars for the schema as
//                                        <name>.json (Grammars2JSON) instead of encoding
//
// With `fragment`, the input's outermost element is a wrapper: its children are the
// fragment's top-level elements.

import com.siemens.ct.exi.core.CodingMode;
import com.siemens.ct.exi.core.EXIFactory;
import com.siemens.ct.exi.core.EncodingOptions;
import com.siemens.ct.exi.core.FidelityOptions;
import com.siemens.ct.exi.core.grammars.Grammars;
import com.siemens.ct.exi.core.helpers.DefaultEXIFactory;
import com.siemens.ct.exi.grammars.GrammarFactory;
import com.siemens.ct.exi.main.api.sax.EXIResult;
import com.siemens.ct.exi.main.api.sax.EXISource;

import java.io.*;
import java.nio.file.*;
import java.util.*;
import javax.xml.parsers.SAXParserFactory;
import javax.xml.transform.TransformerFactory;
import javax.xml.transform.stream.StreamResult;
import org.xml.sax.*;
import org.xml.sax.helpers.XMLFilterImpl;

public class MakeFixtures {
    public static void main(String[] args) throws Exception {
        Path manifest = Paths.get(args[0]);
        Path dir = manifest.getParent();
        Path out = dir.resolve("exificient");
        Files.createDirectories(out);
        int n = 0;
        for (String line : Files.readAllLines(manifest)) {
            if (line.isBlank() || line.startsWith("#")) continue;
            String[] f = line.split("\t");
            String name = f[0];
            Set<String> opts = new HashSet<>(Arrays.asList(f[1].split(",")));
            Path input = dir.resolve("input").resolve(f[2]);
            String schema = f.length > 3 && !f[3].equals("-") ? dir.resolve("input").resolve(f[3]).toString() : null;
            try {
                EXIFactory exi = factory(opts, schema);
                if (opts.contains("grammars")) {
                    try (OutputStream os = Files.newOutputStream(out.resolve(name + ".json"))) {
                        new com.siemens.ct.exi.grammars.persistency.Grammars2JSON()
                            .toGrammarsJSON((com.siemens.ct.exi.core.grammars.SchemaInformedGrammars) exi.getGrammars(), os);
                    }
                } else if (opts.contains("decode")) {
                    decode(exi, input, out.resolve(name + ".xml"));
                } else {
                    encode(exi, opts, input, out.resolve(name + ".exi"));
                }
                n++;
            } catch (Exception e) {
                // A fixture EXIficient refuses is still a result: record why.
                Files.writeString(out.resolve(name + ".error"), e.toString() + "\n");
                System.err.println(name + ": " + e);
            }
        }
        System.out.println(n + " fixtures written to " + out);
    }

    static EXIFactory factory(Set<String> o, String schema) throws Exception {
        EXIFactory exi = DefaultEXIFactory.newInstance();
        if (o.contains("byte")) exi.setCodingMode(CodingMode.BYTE_PACKED);
        if (o.contains("pre")) exi.setCodingMode(CodingMode.PRE_COMPRESSION);
        if (o.contains("compression")) exi.setCodingMode(CodingMode.COMPRESSION);
        FidelityOptions fo = FidelityOptions.createDefault();
        fo.setFidelity(FidelityOptions.FEATURE_STRICT, o.contains("strict"));
        fo.setFidelity(FidelityOptions.FEATURE_PREFIX, o.contains("prefixes"));
        fo.setFidelity(FidelityOptions.FEATURE_COMMENT, o.contains("comments"));
        fo.setFidelity(FidelityOptions.FEATURE_PI, o.contains("pis"));
        fo.setFidelity(FidelityOptions.FEATURE_DTD, o.contains("dtd"));
        fo.setFidelity(FidelityOptions.FEATURE_LEXICAL_VALUE, o.contains("lexical"));
        fo.setFidelity(FidelityOptions.FEATURE_SC, o.contains("sc"));
        exi.setFidelityOptions(fo);
        exi.setFragment(o.contains("fragment"));
        for (String x : o) {
            String[] kv = x.split("=");
            if (kv.length != 2) continue;
            int v = Integer.parseInt(kv[1]);
            switch (kv[0]) {
                case "block": exi.setBlockSize(v); break;
                case "vml": exi.setValueMaxLength(v); break;
                case "vpc": exi.setValuePartitionCapacity(v); break;
                default: throw new IllegalArgumentException("unknown option " + x);
            }
        }
        EncodingOptions eo = exi.getEncodingOptions();
        if (o.contains("options")) eo.setOption(EncodingOptions.INCLUDE_OPTIONS);
        if (o.contains("cookie")) eo.setOption(EncodingOptions.INCLUDE_COOKIE);
        if (o.contains("canonical")) eo.setOption(EncodingOptions.CANONICAL_EXI);
        if (o.contains("utc")) eo.setOption(EncodingOptions.UTC_TIME);
        if (schema != null) {
            Grammars g = GrammarFactory.newInstance().createGrammars(schema);
            exi.setGrammars(g);
        }
        return exi;
    }

    static void encode(EXIFactory exi, Set<String> o, Path input, Path output) throws Exception {
        try (OutputStream os = Files.newOutputStream(output)) {
            EXIResult result = new EXIResult(exi);
            result.setOutputStream(os);
            SAXParserFactory spf = SAXParserFactory.newInstance();
            spf.setNamespaceAware(true);
            XMLReader xr = spf.newSAXParser().getXMLReader();
            ContentHandler h = result.getHandler();
            if (o.contains("fragment")) {
                DropRoot d = new DropRoot();
                d.setContentHandler(h);
                h = d;
            }
            xr.setContentHandler(h);
            xr.setProperty("http://xml.org/sax/properties/lexical-handler", result.getLexicalHandler());
            xr.parse(new InputSource(input.toUri().toString()));
        }
    }

    static void decode(EXIFactory exi, Path input, Path output) throws Exception {
        try (InputStream is = Files.newInputStream(input); OutputStream os = Files.newOutputStream(output)) {
            EXISource src = new EXISource(exi);
            src.setInputSource(new InputSource(is));
            TransformerFactory.newInstance().newTransformer().transform(src, new StreamResult(os));
        }
    }

    /** Passes everything but the outermost element's own start and end. */
    static class DropRoot extends XMLFilterImpl {
        int depth = 0;
        @Override public void startElement(String u, String l, String q, Attributes a) throws SAXException {
            if (depth++ > 0) super.startElement(u, l, q, a);
        }
        @Override public void endElement(String u, String l, String q) throws SAXException {
            if (--depth > 0) super.endElement(u, l, q);
        }
    }
}
