// src/writer/attributes.rs
//
// EXI start tag and attribute writing
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Pending start tags, namespace bindings, and attribute value coding.

use super::{
    DeviationKind, Pending, WriteCode, Writer, XSI_NS, state, unencodable,
};
use crate::coding::{Sink, names};
use crate::engine::{self, Prepared, Slot, err};
use crate::event::NsDecl;
use crate::grammar::informed::TypeId;
use crate::grammar::{EventCode, Terminal};
use crate::name::Name;
use crate::value::{OwnedValue, Value};
use crate::{ErrorKind, NameId, OwnedName, Result};

impl<S: Sink> Writer<'_, S> {
    fn resolve_qname(&self, text: &str) -> Option<OwnedName> {
        let text = text.trim();
        let (prefix, local) = match text.split_once(':') {
            Some((p, l)) => (p, l),
            None => ("", text),
        };
        let uri = match prefix {
            "xml" => "http://www.w3.org/XML/1998/namespace",
            _ => self
                .pending
                .iter()
                .flat_map(|p| {
                    p.ns.iter().rev().map(|(p, u)| (p.as_str(), u.as_str()))
                })
                .chain(
                    self.bindings
                        .iter()
                        .rev()
                        .map(|(p, u, _)| (p.as_str(), u.as_str())),
                )
                .find(|(p, _)| *p == prefix)
                .map(|(_, u)| u)
                .or(if prefix.is_empty() { Some("") } else { None })?,
        };
        let n = OwnedName::new(uri, local);
        Some(if prefix.is_empty() { n } else { n.with_prefix(prefix) })
    }

    /// A value for a QName slot: a QName, or text resolved with the
    /// bindings in scope.
    fn qname_value(&self, v: &Value<'_>) -> Result<OwnedValue> {
        match v {
            Value::QName(n) => Ok(OwnedValue::QName(n.to_owned())),
            _ => {
                let text = engine::lexical_string(v)?;
                self.resolve_qname(&text).map(OwnedValue::QName).ok_or_else(
                    || unencodable("xsi:type value with an undeclared prefix"),
                )
            }
        }
    }

    /// The slot and prepared value of an attribute against the current
    /// state; refuses (`Unencodable`) before anything is written.
    fn prepare_attr<'a>(
        &self,
        t: Terminal,
        code: EventCode,
        n: Name<'_>,
        v: &Value<'a>,
    ) -> Result<(Slot, Prepared<'a>, Option<TypeId>)> {
        let id = match t {
            Terminal::AT(id) => Some(id),
            _ if n.uri() == XSI_NS => {
                self.tables.find_name(n.uri(), n.local_name())
            }
            _ => None,
        }
        .unwrap_or(NameId { uri: u32::MAX, local: u32::MAX });
        let slot = self.st.at_slot(t, code, id);
        let prepared = match slot {
            Slot::QName | Slot::TypeName => {
                let owned = self.qname_value(v)?;
                engine::prepare(self.st.schema, slot, &owned.as_value())?
                    .into_owned()
            }
            _ => engine::prepare(self.st.schema, slot, v)?,
        };
        let ty = if slot == Slot::TypeName {
            let Prepared::QName(uri, local, _) = &prepared else {
                return Err(err(ErrorKind::Internal, "QName expected"));
            };
            Some(self.st.strict_xsi_type(
                uri,
                local,
                ErrorKind::Unencodable,
            )?)
        } else {
            None
        };
        Ok((slot, prepared, ty))
    }

    /// Writes an attribute now, in the current state.
    pub(super) fn write_attr(
        &mut self,
        n: Name<'_>,
        v: &Value<'_>,
    ) -> Result<()> {
        let loose = self.informed_non_strict();
        let informed = matches!(
            self.st.stack.last().map(|f| f.kind),
            Some(engine::FrameKind::Informed { .. })
        );
        if informed && n.is(XSI_NS, "nil") && !self.st.lexical() {
            // As EXIficient, omit a false nil. Non-strict streams can
            // encode non-Boolean nil values as untyped attributes.
            match engine::prepare(None, Slot::Nil, v) {
                Ok(Prepared::Bool(false)) => return Ok(()),
                Err(_) if loose => {
                    let code = self
                        .find_added(Terminal::AtAny, 3)?
                        .ok_or_else(|| {
                            err(
                                ErrorKind::Internal,
                                "no AT(*) [untyped value] production",
                            )
                        })?;
                    return self.write_attr_as(
                        Terminal::AtAny,
                        code,
                        Slot::Str,
                        engine::prepare(None, Slot::Str, v)?,
                        None,
                        n,
                    );
                }
                _ => {}
            }
        }
        let (t, code) =
            self.match_name(n.uri(), n.local_name(), false)?.ok_or_else(
                || unencodable("no production for this attribute here"),
            )?;
        let (t, code, slot, prepared, ty) =
            match self.prepare_attr(t, code.code, n, v) {
                Ok((slot, prepared, ty)) => (t, code, slot, prepared, ty),
                // Not strict: a value its type cannot hold goes
                // untyped, through the declared attribute's AT [untyped
                // value] production, or AT(*)'s.
                Err(e) if e.kind() == ErrorKind::Unencodable && loose => {
                    let (t2, code2) = match self.find_added(t, 3)? {
                        Some(c) if matches!(t, Terminal::AT(_)) => (t, c),
                        _ => (
                            Terminal::AtAny,
                            self.find_added(Terminal::AtAny, 3)?.ok_or(e)?,
                        ),
                    };
                    (
                        t2,
                        code2,
                        Slot::Str,
                        engine::prepare(None, Slot::Str, v)?,
                        None,
                    )
                }
                Err(e) => return Err(e),
            };
        self.write_attr_as(t, code, slot, prepared, ty, n)
    }

    fn write_attr_as(
        &mut self,
        t: Terminal,
        code: WriteCode,
        slot: Slot,
        prepared: Prepared,
        ty: Option<TypeId>,
        n: Name<'_>,
    ) -> Result<()> {
        match (t, code.code.len()) {
            (Terminal::AT(_), 3) => {
                self.deviated(DeviationKind::UntypedValue, code.code, Some(n))
            }
            (Terminal::AtAny | Terminal::AtUri(_), _) if n.uri() != XSI_NS => {
                self.deviated(DeviationKind::Attribute, code.code, Some(n))
            }
            _ => {}
        }
        self.write_code(code)?;
        let (id, _) = self.write_name(t, n)?;
        let prefixes = self.st.prefixes();
        match slot {
            Slot::Nil => {
                let Prepared::Bool(b) = prepared else {
                    return Err(err(ErrorKind::Internal, "Boolean expected"));
                };
                // A typed xsi:nil decides the grammar, so it travels
                // with the structure.
                crate::coding::datatypes::write_bool(
                    &mut self.enc,
                    crate::coding::Channel::Structure,
                    b,
                )?;
                self.st.on_xsi_nil(b)
            }
            Slot::NilText => {
                let Prepared::Str(s, _) = &prepared else {
                    return Err(err(ErrorKind::Internal, "text expected"));
                };
                let b =
                    crate::lexical::parse_bool(s).map(|x| x.0).ok_or_else(
                        || err(ErrorKind::Internal, "Boolean text expected"),
                    )?;
                // Structure, as a typed xsi:nil.
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                self.st.on_xsi_nil(b)
            }
            Slot::TypeName => {
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                self.st.on_xsi_type(Some(ty.ok_or_else(|| {
                    err(ErrorKind::Internal, "type expected")
                })?))
            }
            Slot::QName
                if t == Terminal::AT(engine::XSI_TYPE)
                    && self.st.deviates(code.code) =>
            {
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                let named = match (&prepared, self.st.schema) {
                    (Prepared::QName(uri, local, _), Some(s)) => {
                        s.type_named(uri, local)
                    }
                    _ => None,
                };
                self.st.on_xsi_type(named)
            }
            // A lexical xsi:type is a string, but still part of the
            // structure.
            _ if id == engine::XSI_TYPE => {
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                self.st.on_at(t, code.code, id)
            }
            _ => {
                self.value(id, prepared, prefixes)?;
                self.st.on_at(t, code.code, id)
            }
        }
    }

    /// Writes an NS event now.
    pub(super) fn write_ns(
        &mut self,
        prefix: &str,
        uri: &str,
        local_element_ns: bool,
    ) -> Result<()> {
        let code = self
            .find(Terminal::NS)?
            .ok_or_else(|| state("namespace declaration after content"))?;
        self.write_code(code)?;
        names::write_ns(
            &mut self.enc,
            &mut self.tables,
            NsDecl { prefix, uri, local_element_ns },
        )?;
        self.st.on_misc(Terminal::NS, code.code)
    }

    /// Checks an attribute of a pending start tag against the
    /// declaration it enters.
    pub(super) fn check_pending_attr(
        &self,
        p: &Pending,
        n: Name<'_>,
        v: &Value<'_>,
    ) -> Result<()> {
        let (Some(schema), Some(decl)) = (self.st.schema, p.decl) else {
            return Ok(());
        };
        if !self.cfg.options.strict() {
            return Ok(()); // every attribute can be encoded
        }
        let ed = schema.elem(decl);
        let mut td = schema.ty(ed.ty);
        if n.uri() == XSI_NS && n.local_name() == "type" && td.castable {
            return self.qname_value(v).map(|_| ());
        }
        // An xsi:type already given changes the type the other
        // attributes belong to. When it may still come, the check waits
        // for the start tag to be written.
        let cast =
            p.attrs.iter().find(|(an, _)| an.as_name().is(XSI_NS, "type"));
        match cast {
            Some((_, tv)) => {
                if let Ok(OwnedValue::QName(q)) =
                    self.qname_value(&tv.as_value())
                {
                    if let Some(t) = schema.type_named(
                        q.as_name().uri(),
                        q.as_name().local_name(),
                    ) {
                        td = schema.ty(t);
                    }
                }
            }
            None if td.castable => return Ok(()),
            None => {}
        }
        if n.uri() == XSI_NS && n.local_name() == "nil" {
            // xsi:nil="false" is left out (as EXIficient), so it fits
            // any element.
            let p = engine::prepare(None, Slot::Nil, v);
            if ed.nillable || matches!(p, Ok(Prepared::Bool(false))) {
                return p.map(|_| ());
            }
        }
        let id = self.tables.find_name(n.uri(), n.local_name());
        let uri = self.tables.find_uri_str(n.uri());
        for st in &td.states[..td.content as usize] {
            for pr in st {
                let hit = match pr.term {
                    Terminal::AT(x) => Some(x) == id,
                    Terminal::AtUri(u) => Some(u) == uri,
                    Terminal::AtAny => true,
                    _ => false,
                };
                if hit {
                    return match pr.ty {
                        Some(ty) if !self.st.lexical() => {
                            engine::prepare(Some(schema), Slot::Typed(ty), v)
                                .map(|_| ())
                        }
                        _ => Ok(()),
                    };
                }
            }
        }
        Err(unencodable("attribute not declared for this element"))
    }

    pub(super) fn open_element(
        &mut self,
        term: Terminal,
        code: WriteCode,
        name: Option<Name<'_>>,
    ) -> Result<()> {
        self.write_code(code)?;
        let (id, prefix) = match (term, name) {
            (Terminal::SE(id), None) => (id, None),
            (_, Some(name)) => {
                if matches!(term, Terminal::SeAny | Terminal::SeUri(_)) {
                    self.deviated(
                        DeviationKind::Element,
                        code.code,
                        Some(name),
                    );
                }
                self.write_name(term, name)?
            }
            _ => unreachable!("a wildcard SE keeps its name"),
        };
        self.st.on_se(term, code.code, id, prefix)?;
        self.in_start = true;
        Ok(())
    }

    /// Writes a buffered start tag: SE, NS events, then attributes in
    /// EXI's order.
    pub(super) fn commit(&mut self) -> Result<()> {
        let Some(mut p) = self.pending.take() else { return Ok(()) };
        self.open_element(
            p.term,
            p.code,
            p.name.as_ref().map(OwnedName::as_name),
        )?;
        let depth = self.st.depth();
        for (pre, uri) in &p.ns {
            self.bindings.push((pre.clone(), uri.clone(), depth));
        }
        if self.cfg.canonical {
            p.ns.sort_by(|a, b| a.0.cmp(&b.0));
        }
        if self.st.prefixes() {
            let name = p.name.as_ref().expect("prefixes keep the element name");
            let el_prefix = name.prefix.as_deref().unwrap_or("");
            let mut marked = false;
            for (pre, uri) in core::mem::take(&mut p.ns) {
                let local = !marked && pre == el_prefix && *uri == *name.uri;
                marked |= local;
                self.write_ns(&pre, &uri, local)?;
            }
        }
        // Schema-informed streams sort attributes, undeclared elements'
        // too (EXIficient does; any order is valid in built-in
        // grammars).
        if self.st.schema.is_some() || self.cfg.canonical {
            let priority = |n: &OwnedName| {
                match (&*n.uri, &*n.local) {
                    (XSI_NS, "type") => 0,
                    (XSI_NS, "nil") => 1,
                    _ => 2,
                }
            };
            p.attrs.sort_by(|(a, _), (b, _)| {
                (priority(a), &a.local, &a.uri).cmp(&(priority(b), &b.local, &b.uri))
            });
        }
        for (n, v) in &p.attrs {
            self.write_attr(n.as_name(), &v.as_value())?;
        }
        p.attrs.clear();
        self.spare_attrs = p.attrs;
        Ok(())
    }
}
