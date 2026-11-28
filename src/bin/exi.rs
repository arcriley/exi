// src/bin/exi.rs
//
// Command line exi tool
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! `exi`: the command-line tool for the `exi` crate.
//!
//! Every command is always listed. A command or option that needs a
//! crate feature this build does not have fails with a message naming
//! the feature to build with.
//!
//! ```text
//! cargo build --release --features infer    # everything: xml, xsd, infer
//! exi help
//! ```

use std::io::{BufWriter, Read, Write};
use std::process::ExitCode;

use exi::coding::IoSink;
use exi::grammar::GrammarSet;
use exi::header::parse_header;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{
    PrefixPolicy, XmlConfig, XmlReader, XmlWriter, XmlWriterConfig,
};
use exi::{
    Coding, Header, Limits, Options, Preserve, Reader, ReaderConfig, SchemaId,
    Status, Writer, WriterConfig,
};

const USAGE: &str = "\
exi: Efficient XML Interchange (W3C EXI 1.0) from the command line

usage:
  exi encode [options] [IN.xml] [-o OUT.exi]     XML text to EXI
  exi decode [options] [IN.exi] [-o OUT.xml]     EXI to XML text
  exi info IN.exi                                the header: options and schema
  exi compile MAIN.xsd [MORE.xsd ...] -o OUT.exig    XSD to grammars      (feature xsd)
  exi infer [--exact] [--max-enum N] [-d DIR] [-o OUT.exig] DOC.xml ...
                                                 learn a schema from examples (feature infer)
  exi exig IN.exig [-o OUT.xml]                  an .exig file as XML text
  exi xhtml PAGE.html [-o OUT.xhtml]             an HTML page as XHTML      (feature html5ever)
  exi xhtml5 report|suggest [--top N] PAGE.html ... [-o OUT]
                                                 what pages need that the XHTML5 schema
                                                 lacks, the N most common (default 40)
                                                                            (feature xhtml5)
  exi digest SCHEMA ...                          the sha256: name of a schema's grammars
  exi features                                   what this build can do
  exi help | --version

IN and OUT default to standard input and output; `-` names them too.
Options that take a value also take it as `--name=value`.

encode and decode options:
  -s, --schema FILE    an .exig file, or .xsd files (the first is the main one; the
                       others are there for its includes and imports). Repeat for more.
                       With a schema the stream is strict and names the schema's digest.
      --non-strict     with a schema: not strict, so content the schema does not
                       declare still encodes. Any --preserve item then works.
  -p, --preserve LIST  comma-separated: comments, pis, dtd, prefixes, lexical, all.
                       Default none: the smallest stream. Strict streams take only
                       `lexical`.
      --bare           encode: no options in the header (the reader must be told them).
                       decode: the options to assume for a header without them.
      --cookie         encode: start with the `$EXI` cookie.
  -c, --coding NAME    byte (default), bit, pre or compression: byte-aligned,
                       bit-packed, pre-compression (values grouped by name, ready
                       for an outside compressor) or compression (pre-compression
                       and DEFLATE).

decode uses -c, -p, --block and --non-strict only for headers without options,
and says so when the header has them.
      --block N        pre and compression: values per block (default 1000000).
      --level N        compression: the DEFLATE level, 0 to 9 (default 6).
      --canonical      encode: Canonical EXI (EXI C14N): one stream for given events,
                       options and schema. Not with the cookie or -c compression.
      --utc-time       encode, with --canonical: date-times in UTC.
      --html           encode: the input is HTML text, parsed as browsers do.
                       decode: write HTML text.                    (feature html5ever)
      --xhtml5         with the XHTML5 profile's schema and options. encode: the
                       header names the profile, unless --bare. decode: for bare
                       headers; a header that names the profile needs no flag.
                                                                   (feature xhtml5)

The default body is byte-aligned: ready for zstd, xz or gzip, which do better on it
than on bit-packed EXI or on the XML text. `-c pre` groups values by name first, which
helps those compressors more; `-c compression` is EXI's own DEFLATE.

  exi encode logo.svg | zstd -19 > logo.svg.exi.zst
  zstd -dc logo.svg.exi.zst | exi decode > logo.svg
";

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("exi: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Res<()> {
    let Some(cmd) = args.first() else {
        return Err(format!("no command given\n\n{USAGE}").into());
    };
    let a = Args::parse(&args[1..])?;
    match cmd.as_str() {
        "encode" => encode(&a),
        "decode" => decode(&a),
        "info" => info(&a),
        "compile" => compile(&a),
        "infer" => infer(&a),
        "exig" => exig_xml(&a),
        "xhtml" => xhtml(&a),
        "xhtml5" => xhtml5(&a),
        "digest" => digest(&a),
        "features" => {
            features();
            Ok(())
        }
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "version" => {
            println!("exi {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => {
            Err(format!("unknown command `{other}` (try `exi help`)").into())
        }
    }
}

// ----- arguments -----

#[derive(Default)]
struct Args {
    files: Vec<String>,
    out: Option<String>,
    dir: Option<String>,
    schemas: Vec<String>,
    preserve: Preserve,
    bare: bool,
    non_strict: bool,
    html: bool,
    xhtml5: bool,
    top: Option<usize>,
    cookie: bool,
    coding: Option<Coding>,
    block: Option<u32>,
    level: Option<u32>,
    canonical: bool,
    utc_time: bool,
    exact: bool,
    max_enum: Option<usize>,
}

impl Args {
    fn parse(args: &[String]) -> Res<Args> {
        let mut a = Args::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            // `--name=value` is `--name value`.
            let (x, mut inline) = match arg.split_once('=') {
                Some((k, v)) if k.starts_with("--") => (k, Some(v.to_owned())),
                _ => (arg.as_str(), None),
            };
            let mut value = |name: &str| match inline.take() {
                Some(v) => Ok(v),
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value")),
            };
            match x {
                "-o" | "--output" => a.out = Some(value(x)?),
                "-d" | "--dir" => a.dir = Some(value(x)?),
                "-s" | "--schema" => a.schemas.push(value(x)?),
                "-p" | "--preserve" => a.preserve = preserve(&value(x)?)?,
                "--bare" => a.bare = true,
                "--non-strict" => a.non_strict = true,
                "--html" => a.html = true,
                "--xhtml5" => a.xhtml5 = true,
                "--top" => {
                    a.top = Some(
                        value(x)?
                            .parse()
                            .map_err(|_| "--top needs a number")?,
                    )
                }
                "--cookie" => a.cookie = true,
                "-c" | "--coding" => a.coding = Some(coding(&value(x)?)?),
                "--canonical" => a.canonical = true,
                "--utc-time" => a.utc_time = true,
                "--block" => {
                    a.block = Some(
                        value(x)?
                            .parse()
                            .map_err(|_| "--block needs a number from 1")?,
                    )
                }
                "--level" => {
                    a.level =
                        Some(value(x)?.parse().map_err(
                            |_| "--level needs a number from 0 to 9",
                        )?)
                }
                "--exact" => a.exact = true,
                "--max-enum" => {
                    a.max_enum = Some(
                        value(x)?
                            .parse()
                            .map_err(|_| "--max-enum needs a number")?,
                    )
                }
                "-" => a.files.push(arg.clone()),
                s if s.starts_with('-') => {
                    return Err(format!(
                        "unknown option `{s}` (try `exi help`)"
                    )
                    .into());
                }
                _ => a.files.push(arg.clone()),
            }
            if inline.is_some() {
                return Err(format!("{x} takes no value").into());
            }
        }
        Ok(a)
    }

    /// The one input file, or standard input.
    fn input(&self) -> Res<Box<dyn Read>> {
        match self.files.as_slice() {
            [] => Ok(Box::new(std::io::stdin().lock())),
            [f] if f == "-" => Ok(Box::new(std::io::stdin().lock())),
            [f] => Ok(Box::new(
                std::fs::File::open(f).map_err(|e| format!("{f}: {e}"))?,
            )),
            _ => Err("give one input file".into()),
        }
    }

    fn output(&self) -> Res<Box<dyn Write>> {
        match self.out.as_deref() {
            None | Some("-") => {
                Ok(Box::new(BufWriter::new(std::io::stdout().lock())))
            }
            Some(f) => Ok(Box::new(BufWriter::new(
                std::fs::File::create(f).map_err(|e| format!("{f}: {e}"))?,
            ))),
        }
    }
}

fn coding(name: &str) -> Res<Coding> {
    Ok(match name {
        "byte" | "byte-aligned" => Coding::ByteAligned,
        "bit" | "bit-packed" => Coding::BitPacked,
        "pre" | "pre-compression" => Coding::PreCompression,
        "compression" | "deflate" => Coding::Compression,
        _ => {
            return Err(format!(
                "unknown --coding `{name}`: use byte, bit, pre or compression"
            )
            .into());
        }
    })
}

impl Args {
    /// Options from the flags: coding, strict, preserve, block size.
    fn options(&self, informed: bool) -> exi::OptionsBuilder {
        let b = Options::builder()
            .coding(self.coding.unwrap_or(Coding::ByteAligned))
            .strict(informed && !self.non_strict)
            .preserve(self.preserve);
        match self.block {
            Some(n) => b.block_size(n),
            None => b,
        }
    }
    /// `o` with the coding and block size the flags give, when they
    /// give them.
    fn recoded(&self, o: Options) -> Res<Options> {
        let mut b = o.to_builder();
        if let Some(c) = self.coding {
            b = b.coding(c);
        }
        if let Some(n) = self.block {
            b = b.block_size(n);
        }
        Ok(b.build()?)
    }
    /// The flags given that only describe a header without options.
    fn header_flags(&self) -> Vec<&'static str> {
        [
            (self.coding.is_some(), "--coding"),
            (self.preserve != Preserve::empty(), "--preserve"),
            (self.block.is_some(), "--block"),
            (self.non_strict, "--non-strict"),
            (self.bare, "--bare"),
        ]
        .into_iter()
        .filter_map(|(given, flag)| given.then_some(flag))
        .collect()
    }
    fn writer_config<'g>(
        &self,
        opts: Options,
        g: &'g exi::grammar::Grammars<'g>,
        header: Header,
    ) -> WriterConfig<'g> {
        let c = WriterConfig::new(opts, g)
            .header(header)
            .canonical(self.canonical)
            .utc_time(self.utc_time);
        match self.level {
            Some(l) => c.deflate_level(l),
            None => c,
        }
    }
}

fn preserve(list: &str) -> Res<Preserve> {
    let mut p = Preserve::empty();
    for w in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        p |= match w {
            "comments" => Preserve::COMMENTS,
            "pis" => Preserve::PIS,
            "dtd" => Preserve::DTD,
            "prefixes" => Preserve::PREFIXES,
            "lexical" => Preserve::LEXICAL_VALUES,
            "all" => Preserve::all(),
            _ => return Err(format!("unknown --preserve item `{w}`: use comments, pis, dtd, prefixes, lexical or all").into()),
        };
    }
    Ok(p)
}

/// A message for a feature this build does not have.
#[allow(dead_code)]
fn needs(feature: &str, what: &str) -> Box<dyn std::error::Error> {
    format!("{what} needs the `{feature}` feature, which this build does not have; build with `cargo build --release --features {feature}`").into()
}

// ----- schemas -----

/// Grammars from `-s` files: one `.exig`, or `.xsd` files with the
/// first as the main one.
fn load_schema(files: &[String]) -> Res<GrammarSet> {
    let read = |f: &String| std::fs::read(f).map_err(|e| format!("{f}: {e}"));
    let is_exig = |f: &String, b: &[u8]| {
        f.ends_with(".exig")
            || b.starts_with(b"$EXI")
            || b.first().is_some_and(|x| x & 0xC0 == 0x80)
    };
    let docs: Vec<(&String, Vec<u8>)> =
        files.iter().map(|f| Ok((f, read(f)?))).collect::<Res<_>>()?;
    match docs.as_slice() {
        [] => Err("no schema given".into()),
        [(f, b)] if is_exig(f, b) => {
            Ok(GrammarSet::from_exig(b).map_err(|e| format!("{f}: {e}"))?)
        }
        _ if docs.iter().any(|(f, b)| is_exig(f, b)) => {
            Err("give one .exig file, or .xsd files, not both".into())
        }
        _ => compile_xsd(&docs),
    }
}

#[cfg(feature = "xsd")]
fn compile_xsd(docs: &[(&String, Vec<u8>)]) -> Res<GrammarSet> {
    let mut c = exi::xsd::XsdCompiler::new();
    for (f, b) in docs {
        c = c.document(f, b);
    }
    Ok(c.compile(docs[0].0).map_err(|e| format!("{}: {e}", docs[0].0))?)
}

#[cfg(not(feature = "xsd"))]
fn compile_xsd(_: &[(&String, Vec<u8>)]) -> Res<GrammarSet> {
    Err(needs("xsd", "reading .xsd files"))
}

// ----- encode and decode -----

const CHUNK: usize = 64 * 1024;

fn encode(a: &Args) -> Res<()> {
    if a.html || a.xhtml5 {
        return encode_html(a);
    }
    let set = if a.schemas.is_empty() {
        GrammarSet::schemaless()
    } else {
        load_schema(&a.schemas)?
    };
    let informed = !a.schemas.is_empty();
    let mut b = a.options(informed);
    if informed && !a.bare {
        b = b.schema_id(SchemaId::Id(set.digest()?.into()));
    }
    let opts = b.build()?;
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    let cfg = XmlConfig::new()
        .report_comments(a.preserve.contains(Preserve::COMMENTS))
        .report_pis(a.preserve.contains(Preserve::PIS))
        .report_doctype(a.preserve.contains(Preserve::DTD));
    let mut x: XmlReader = XmlReader::new(cfg, &g);
    let header = if a.bare {
        Header::Bare { cookie: a.cookie }
    } else {
        Header::WithOptions { cookie: a.cookie }
    };
    let mut w =
        Writer::new(IoSink(a.output()?), a.writer_config(opts, &g, header))?;
    let mut input = a.input()?;
    let mut buf = vec![0u8; CHUNK];
    let (mut at, mut len) = (0, 0);
    loop {
        if at == len {
            len = input.read(&mut buf)?;
            at = 0;
            if len == 0 {
                x.end_input();
            }
        }
        at += x.feed(&buf[at..len]);
        if exi::transcode(&mut x, &mut w)
            .map_err(|e| format!("{e} (at {:?})", x.position()))?
            == Status::End
        {
            break;
        }
    }
    w.into_sink().0.flush()?;
    Ok(())
}

/// The options to assume for a header without them: the encode options
/// given.
fn assumed(a: &Args, informed: bool) -> Res<Options> {
    Ok(a.options(informed).build()?)
}

fn decode(a: &Args) -> Res<()> {
    let mut input = a.input()?;
    // Read until the header is complete: the schema it names decides
    // the grammars.
    let mut data = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let header = loop {
        let n = input.read(&mut buf)?;
        data.extend_from_slice(&buf[..n]);
        match parse_header(&data, &Limits::new())? {
            Some(h) => break h,
            None if n == 0 => {
                return Err("the input ends inside the EXI header".into());
            }
            None => {}
        }
    };
    let named = match header.options.as_ref().map(|o| o.schema_id()) {
        Some(SchemaId::Id(id)) => Some(id.to_string()),
        _ => None,
    };
    let profile = profile_version(named.as_deref(), a)?;
    let owned;
    let set: &GrammarSet = match profile {
        Some(v) => profile_grammars(v)?,
        None => {
            owned = decode_grammars(&header, a)?;
            &owned
        }
    };
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    let mut cfg = ReaderConfig::new(&g);
    let mut fragment = header.options.as_ref().is_some_and(|o| o.fragment());
    if header.options.is_some() {
        let ignored = a.header_flags();
        if !ignored.is_empty() {
            eprintln!(
                "exi: the stream's header has its options; ignoring {}",
                ignored.join(", ")
            );
        }
    } else {
        let opts = match profile {
            Some(v) => a.recoded(profile_options(v)?)?,
            None => assumed(a, !a.schemas.is_empty())?,
        };
        fragment = opts.fragment();
        cfg = cfg.options(opts);
    }
    let mut r: Reader = Reader::new(cfg)?;
    let html = a.html;
    let mut w = if html {
        html_writer(a)?
    } else {
        // With prefixes in the stream, write the document's own
        // declarations and prefixes.
        let kept = header
            .options
            .as_ref()
            .map_or(a.preserve, |o| o.preserve())
            .contains(Preserve::PREFIXES);
        let policy = if kept {
            PrefixPolicy::FromEvents
        } else {
            PrefixPolicy::Generate
        };
        Out::Xml(XmlWriter::new(
            IoSink(a.output()?),
            XmlWriterConfig::new()
                .declaration(true)
                .prefixes(policy)
                .fragment(fragment),
        ))
    };
    let mut at = 0;
    let mut ended = false;
    loop {
        at += r.feed(&data[at..]);
        if at == data.len() && !ended {
            let n = input.read(&mut buf)?;
            if n == 0 {
                r.end_input();
                ended = true;
            } else {
                data.clear();
                data.extend_from_slice(&buf[..n]);
                at = 0;
            }
        }
        if exi::transcode(&mut r, &mut w)
            .map_err(|e| format!("{e} (at {:?})", r.position()))?
            == Status::End
        {
            break;
        }
    }
    w.finish()
}

type OutSink = IoSink<Box<dyn Write>>;

/// Where decoded events go: XML text, or HTML text.
enum Out {
    Xml(XmlWriter<OutSink>),
    #[cfg(feature = "html")]
    Html(exi::html::HtmlWriter<OutSink>),
}

impl exi::EventSink for Out {
    fn event(&mut self, e: &exi::Event<'_>) -> exi::Result<()> {
        match self {
            Out::Xml(w) => w.event(e),
            #[cfg(feature = "html")]
            Out::Html(w) => w.event(e),
        }
    }
}

impl Out {
    fn finish(self) -> Res<()> {
        match self {
            Out::Xml(w) => w.into_sink().0.flush()?,
            #[cfg(feature = "html")]
            Out::Html(w) => w.into_sink().0.flush()?,
        }
        Ok(())
    }
}

/// The grammars a stream's header and the `-s` files give.
fn decode_grammars(
    header: &exi::header::ParsedHeader,
    a: &Args,
) -> Res<GrammarSet> {
    Ok(
        match (header.options.as_ref().map(|o| o.schema_id()), a.schemas.is_empty()) {
        (Some(SchemaId::Id(id)), true) => return Err(format!("the stream needs the schema `{id}`: give it with -s").into()),
        (Some(SchemaId::Id(id)), false) => {
            let set = load_schema(&a.schemas)?;
            let d = set.digest()?;
            if id.starts_with("sha256:") && **id != *d {
                return Err(format!("the stream names the schema `{id}`, but the schema given is `{d}`").into());
            }
            set
        }
        (_, true) => GrammarSet::schemaless(),
        (Some(o), false) => return Err(format!("the stream is schema-less (schemaId {o:?}), but a schema was given").into()),
        (None, false) => load_schema(&a.schemas)?,
    },
    )
}

fn info(a: &Args) -> Res<()> {
    let mut data = Vec::new();
    a.input()?.take(1 << 16).read_to_end(&mut data)?;
    let h = parse_header(&data, &Limits::new())?
        .ok_or("the input ends inside the EXI header")?;
    println!("cookie: {}", if h.cookie { "yes" } else { "no" });
    println!("header: {} bits", h.body_offset_bits);
    match &h.options {
        None => println!("options: not in the stream (agreed out of band)"),
        Some(o) => {
            println!("coding: {:?}", o.coding());
            println!("strict: {}", o.strict());
            println!("fragment: {}", o.fragment());
            println!("self-contained: {}", o.self_contained());
            let p = o.preserve();
            let names: Vec<&str> = [
                (Preserve::COMMENTS, "comments"),
                (Preserve::PIS, "pis"),
                (Preserve::DTD, "dtd"),
                (Preserve::PREFIXES, "prefixes"),
                (Preserve::LEXICAL_VALUES, "lexical"),
            ]
            .iter()
            .filter(|(f, _)| p.contains(*f))
            .map(|(_, n)| *n)
            .collect();
            println!(
                "preserve: {}",
                if names.is_empty() { "none".into() } else { names.join(",") }
            );
            match o.schema_id() {
                SchemaId::Absent => println!("schema: not named"),
                SchemaId::Nil => println!("schema: none (schema-less)"),
                SchemaId::BuiltinTypes => {
                    println!("schema: built-in types only")
                }
                SchemaId::Id(id) => println!("schema: {id}"),
            }
            if matches!(
                o.coding(),
                Coding::Compression | Coding::PreCompression
            ) {
                println!("block size: {}", o.block_size());
            }
            if let Some(n) = o.value_max_length() {
                println!("value max length: {n}");
            }
            if let Some(n) = o.value_partition_capacity() {
                println!("value partition capacity: {n}");
            }
        }
    }
    Ok(())
}

// ----- schemas: compile, infer, exig, digest -----

#[cfg(feature = "xsd")]
fn compile(a: &Args) -> Res<()> {
    if a.files.is_empty() {
        return Err("give the .xsd files to compile".into());
    }
    let out =
        a.out.as_deref().ok_or("give the output file with -o OUT.exig")?;
    let docs: Vec<(&String, Vec<u8>)> = a
        .files
        .iter()
        .map(|f| Ok((f, std::fs::read(f).map_err(|e| format!("{f}: {e}"))?)))
        .collect::<Res<_>>()?;
    let set = compile_xsd(&docs)?;
    write_exig(&set, out)
}

#[cfg(not(feature = "xsd"))]
fn compile(_: &Args) -> Res<()> {
    Err(needs("xsd", "exi compile"))
}

/// Writes `set` as `.exig` (`-` for standard output) and reports its
/// digest.
#[allow(dead_code)]
fn write_exig(set: &GrammarSet, out: &str) -> Res<()> {
    let bytes = set.to_exig()?;
    if out == "-" {
        std::io::stdout().lock().write_all(&bytes)?;
    } else {
        std::fs::write(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    }
    eprintln!("{out}: {} octets, {}", bytes.len(), set.digest()?);
    Ok(())
}

#[cfg(feature = "infer")]
fn infer(a: &Args) -> Res<()> {
    use exi::infer::{Fit, SchemaInference};
    use std::path::Path;
    if a.files.is_empty() {
        return Err("give one or more example documents".into());
    }
    let mut learn = SchemaInference::new().fit(if a.exact {
        Fit::Exact
    } else {
        Fit::General
    });
    if let Some(n) = a.max_enum {
        learn = learn.max_enumeration(n);
    }
    for f in &a.files {
        let mut b = Vec::new();
        if f == "-" {
            std::io::stdin().lock().read_to_end(&mut b)?;
        } else {
            b = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
        }
        learn.document(&b).map_err(|e| format!("{f}: {e}"))?;
    }
    let docs = learn.schemas();
    if let Some(dir) = &a.dir {
        std::fs::create_dir_all(dir)?;
        for d in &docs {
            let p = Path::new(dir).join(&d.location);
            std::fs::write(&p, &d.text)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            eprintln!("{}: namespace `{}`", p.display(), d.namespace);
        }
    }
    if let Some(out) = &a.out {
        write_exig(&learn.grammars()?, out)?;
    }
    if a.dir.is_none() && a.out.is_none() {
        // Standard output: the one schema document; more need a
        // directory.
        match docs.as_slice() {
            [d] => print!("{}", d.text),
            _ => return Err(format!("the examples use {} namespaces, one schema document each: give a directory with -d", docs.len()).into()),
        }
    }
    Ok(())
}

#[cfg(not(feature = "infer"))]
fn infer(_: &Args) -> Res<()> {
    Err(needs("infer", "exi infer"))
}

fn exig_xml(a: &Args) -> Res<()> {
    let mut file = Vec::new();
    a.input()?.read_to_end(&mut file)?;
    let set = exi::grammar::exig::exig_grammars()?;
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    let mut r: Reader = Reader::new(ReaderConfig::new(&g))?;
    let mut w = XmlWriter::new(
        IoSink(a.output()?),
        XmlWriterConfig::new().declaration(true),
    );
    let mut rest = &file[..];
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        if exi::transcode(&mut r, &mut w)? == Status::End {
            break;
        }
    }
    w.into_sink().0.flush()?;
    Ok(())
}

fn digest(a: &Args) -> Res<()> {
    if a.files.is_empty() {
        return Err("give an .exig file, or .xsd files".into());
    }
    println!("{}", load_schema(&a.files)?.digest()?);
    Ok(())
}

fn features() {
    let f = [
        ("std", cfg!(feature = "std"), "standard library: files, IoSink"),
        (
            "xml",
            cfg!(feature = "xml"),
            "XML text reader and writer: encode, decode",
        ),
        ("xsd", cfg!(feature = "xsd"), "XSD compiler: compile, -s FILE.xsd"),
        (
            "infer",
            cfg!(feature = "infer"),
            "schemas learned from examples: infer",
        ),
        (
            "bit-packed",
            cfg!(feature = "bit-packed"),
            "bit-packed bodies: --coding bit",
        ),
        (
            "compression",
            cfg!(feature = "compression"),
            "pre-compression and DEFLATE: --coding pre, --coding compression",
        ),
        (
            "dom",
            cfg!(feature = "dom"),
            "layer 2, the document tree (libxml2's tree API)",
        ),
        ("html", cfg!(feature = "html"), "HTML out: decode --html"),
        (
            "html5ever",
            cfg!(feature = "html5ever"),
            "HTML in: encode --html, xhtml",
        ),
        (
            "xhtml5",
            cfg!(feature = "xhtml5"),
            "the XHTML5 profile: --xhtml5, xhtml5 report|suggest",
        ),
        ("capi", cfg!(feature = "capi"), "the C ABI (libexi.so)"),
    ];
    for (name, on, what) in f {
        println!("{} {name:11} {what}", if on { "+" } else { "-" });
    }
    let codings = [
        ("byte-aligned", true),
        ("bit-packed", cfg!(feature = "bit-packed")),
        ("pre-compression", cfg!(feature = "compression")),
        ("compression", cfg!(feature = "compression")),
    ];
    let built: Vec<&str> =
        codings.iter().filter(|c| c.1).map(|c| c.0).collect();
    println!("codings: {}", built.join(", "));
}

// ----- HTML: encode --html, decode --html, xhtml, xhtml5 -----

/// The XHTML5 profile version for this stream: the one its header
/// names, or 1 with `--xhtml5` for a bare header.
fn profile_version(named: Option<&str>, a: &Args) -> Res<Option<u32>> {
    #[cfg(feature = "xhtml5")]
    {
        if let Some(v) = named.and_then(exi::html::profile::version_of) {
            return Ok(Some(v));
        }
        Ok(a.xhtml5.then_some(exi::html::profile::LATEST))
    }
    #[cfg(not(feature = "xhtml5"))]
    {
        if a.xhtml5
            || named.is_some_and(|n| n.starts_with("urn:libexi:xhtml5:"))
        {
            return Err(needs("xhtml5", "the XHTML5 profile"));
        }
        Ok(None)
    }
}

#[cfg(feature = "xhtml5")]
fn profile_grammars(v: u32) -> Res<&'static GrammarSet> {
    Ok(exi::html::profile::grammars(v)
        .ok_or("no such XHTML5 profile version")?)
}

#[cfg(not(feature = "xhtml5"))]
fn profile_grammars(_: u32) -> Res<&'static GrammarSet> {
    Err(needs("xhtml5", "the XHTML5 profile"))
}

#[cfg(feature = "xhtml5")]
fn profile_options(v: u32) -> Res<Options> {
    Ok(exi::html::profile::options(v)?)
}

#[cfg(not(feature = "xhtml5"))]
fn profile_options(_: u32) -> Res<Options> {
    Err(needs("xhtml5", "the XHTML5 profile"))
}

#[cfg(feature = "html")]
fn html_writer(a: &Args) -> Res<Out> {
    Ok(Out::Html(exi::html::HtmlWriter::new(IoSink(a.output()?))))
}

#[cfg(not(feature = "html"))]
fn html_writer(_: &Args) -> Res<Out> {
    Err(needs("html", "HTML output"))
}

#[cfg(feature = "html5ever")]
fn read_text(a: &Args) -> Res<String> {
    let mut b = Vec::new();
    a.input()?.read_to_end(&mut b)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

#[cfg(feature = "html5ever")]
fn encode_html(a: &Args) -> Res<()> {
    let page = read_text(a)?;
    let header = if a.bare {
        Header::Bare { cookie: a.cookie }
    } else {
        Header::WithOptions { cookie: a.cookie }
    };
    let owned;
    let (set, opts): (&GrammarSet, Options) = match profile_version(None, a)? {
        Some(v) => {
            if a.preserve != Preserve::empty() || !a.schemas.is_empty() {
                return Err(
                    "--xhtml5 fixes the schema and options: leave out -s and --preserve".into(),
                );
            }
            (profile_grammars(v)?, a.recoded(profile_options(v)?)?)
        }
        None => {
            owned = if a.schemas.is_empty() {
                GrammarSet::schemaless()
            } else {
                load_schema(&a.schemas)?
            };
            let informed = !a.schemas.is_empty();
            let mut b = a.options(informed);
            if informed && !a.bare {
                b = b.schema_id(SchemaId::Id(owned.digest()?.into()));
            }
            (&owned, b.build()?)
        }
    };
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    let mut w =
        Writer::new(IoSink(a.output()?), a.writer_config(opts, &g, header))?;
    let html_opts = exi::html::HtmlOptions::new()
        .keep_doctype(a.preserve.contains(Preserve::DTD));
    exi::html::encode_html(&page, &mut w, &html_opts)?;
    w.into_sink().0.flush()?;
    Ok(())
}

#[cfg(not(feature = "html5ever"))]
fn encode_html(_: &Args) -> Res<()> {
    Err(needs("html5ever", "reading HTML text"))
}

#[cfg(feature = "html5ever")]
fn xhtml(a: &Args) -> Res<()> {
    let page = read_text(a)?;
    let x = exi::html::html_to_xhtml(&page)?;
    let mut out = a.output()?;
    out.write_all(x.as_bytes())?;
    out.flush()?;
    Ok(())
}

#[cfg(not(feature = "html5ever"))]
fn xhtml(_: &Args) -> Res<()> {
    Err(needs("html5ever", "exi xhtml"))
}

/// `exi xhtml5 report|suggest PAGE.html ...`
#[cfg(all(feature = "xhtml5", feature = "html5ever"))]
fn xhtml5(a: &Args) -> Res<()> {
    use exi::{DeviationKind, Deviations};
    let (cmd, pages) = a
        .files
        .split_first()
        .ok_or("exi xhtml5 report|suggest PAGE.html ...")?;
    if pages.is_empty() {
        return Err("give one or more pages".into());
    }
    let mut all = Deviations::default();
    for p in pages {
        let mut b = Vec::new();
        if p == "-" {
            std::io::stdin().lock().read_to_end(&mut b)?;
        } else {
            b = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
        }
        all.merge(
            &exi::html::profile::report(
                &String::from_utf8_lossy(&b),
                exi::html::profile::LATEST,
            )
            .map_err(|e| format!("{p}: {e}"))?,
        );
    }
    let local = |c: &str| c.rsplit('}').next().unwrap_or(c).to_string();
    let top = a.top.unwrap_or(40);
    let mut out = a.output()?;
    match cmd.as_str() {
        "report" => {
            writeln!(out, "{:>8}  {:14} {:24} name", "count", "kind", "in element")?;
            for (k, el, name, n) in all.iter().take(top) {
                writeln!(
                out,
                    "{n:>8}  {:14} {:24} {name}",
                    format!("{k:?}"),
                    local(el)
                )?;
            }
            writeln!(out, "{} in all, {} pages", all.total(), pages.len())?;
        }
        "suggest" => {
            // Names the next version could declare: elements in the
            // XHTML namespace, and attributes other than data-* and
            // aria-* (AT(*) is meant for those).
            let mut elements: std::collections::BTreeMap<String, u64> =
                Default::default();
            let mut attrs: std::collections::BTreeMap<(String, String), u64> =
                Default::default();
            for (k, el, name, n) in all.iter() {
                match k {
                    DeviationKind::Element
                        if name
                            .starts_with("{http://www.w3.org/1999/xhtml}") =>
                    {
                        *elements.entry(local(name)).or_default() += n
                    }
                    DeviationKind::Attribute
                        if name.starts_with("{}")
                            && !name.starts_with("{}data-")
                            && !name.starts_with("{}aria-") =>
                    {
                        *attrs.entry((local(name), local(el))).or_default() +=
                            n
                    }
                    _ => {}
                }
            }
            let mut e: Vec<_> = elements.into_iter().collect();
            e.sort_by_key(|x| std::cmp::Reverse(x.1));
            writeln!(
                out,
                "<!-- elements to declare (count): add a global element and a ref in the Flow choice -->"
            )?;
            for (name, n) in e.iter().take(top) {
                writeln!(
                out,
                    "<xs:element name=\"{name}\" type=\"Flow\"/>  <!-- {n} -->"
                )?;
            }
            let mut by_attr: std::collections::BTreeMap<
                String,
                (u64, Vec<String>),
            > = Default::default();
            for ((name, el), n) in attrs {
                let x = by_attr.entry(name).or_default();
                x.0 += n;
                x.1.push(el);
            }
            let mut v: Vec<_> = by_attr.into_iter().collect();
            v.sort_by_key(|x| std::cmp::Reverse(x.1.0));
            writeln!(
                out,
                "<!-- attributes to declare (count; elements): on the element's type, or in the global group when on many -->"
            )?;
            for (name, (n, els)) in v.iter().take(top) {
                writeln!(
                out,
                    "<xs:attribute name=\"{name}\" type=\"xs:string\"/>  <!-- {n}; {} -->",
                    els.join(" ")
                )?;
            }
        }
        _ => {
            return Err(format!(
                "unknown xhtml5 command `{cmd}`: report or suggest"
            )
            .into());
        }
    }
    out.flush()?;
    Ok(())
}

#[cfg(not(all(feature = "xhtml5", feature = "html5ever")))]
fn xhtml5(_: &Args) -> Res<()> {
    Err(needs("xhtml5,html5ever", "exi xhtml5"))
}
