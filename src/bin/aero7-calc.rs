//! Calculator – Windows 7 Calculator remake (Standard / Scientific / Programmer).
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aero7::{config, sounds, theme, util};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Standard, Scientific, Programmer }

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op { Add, Sub, Mul, Div, Pow, Root, Mod, And, Or, Xor, Lsh, Rsh }

impl Op {
    fn sym(self) -> &'static str {
        match self {
            Op::Add => "+", Op::Sub => "-", Op::Mul => "*", Op::Div => "/", Op::Pow => "^", Op::Root => "yroot",
            Op::Mod => "Mod", Op::And => "And", Op::Or => "Or", Op::Xor => "Xor", Op::Lsh => "Lsh", Op::Rsh => "Rsh",
        }
    }
    fn prec(self) -> u8 {
        match self {
            Op::Or => 1, Op::Xor => 2, Op::And => 3, Op::Lsh | Op::Rsh => 4, Op::Add | Op::Sub => 5,
            Op::Mul | Op::Div | Op::Mod => 6, Op::Pow | Op::Root => 7,
        }
    }
}

#[derive(Clone, Debug)]
enum Tok { Num(f64), Op(Op), LParen, RParen }

#[derive(Clone, Copy, PartialEq)]
enum Base { Hex, Dec, Oct, Bin }
#[derive(Clone, Copy, PartialEq)]
enum Word { Qword, Dword, Word, Byte }

struct Calc {
    mode: Mode,
    entry: String,        // what the user is typing / last result text
    fresh: bool,          // next digit starts a new number
    toks: Vec<Tok>,       // pending expression
    expr_text: String,    // the small line above the number
    last: Option<(Op, f64)>, // for repeated "="
    memory: f64,
    has_mem: bool,
    error: bool,
    inv: bool,
    angle: u8,            // 0 deg, 1 rad, 2 grad
    base: Base,
    word: Word,
    grouping: bool,
    history: Vec<String>,
    pending_unary: Option<String>,
}

impl Calc {
    fn new() -> Calc {
        Calc { mode: Mode::Standard, entry: "0".into(), fresh: true, toks: vec![], expr_text: String::new(), last: None,
            memory: 0.0, has_mem: false, error: false, inv: false, angle: 0, base: Base::Dec, word: Word::Qword,
            grouping: false, history: vec![], pending_unary: None }
    }

    fn mask(&self) -> u64 { match self.word { Word::Qword => u64::MAX, Word::Dword => 0xffff_ffff, Word::Word => 0xffff, Word::Byte => 0xff } }
    fn bits(&self) -> u32 { match self.word { Word::Qword => 64, Word::Dword => 32, Word::Word => 16, Word::Byte => 8 } }

    fn value(&self) -> f64 {
        if self.mode == Mode::Programmer {
            let radix = match self.base { Base::Hex => 16, Base::Dec => 10, Base::Oct => 8, Base::Bin => 2 };
            let t = self.entry.replace([' ', ','], "");
            let neg = t.starts_with('-');
            let v = u64::from_str_radix(t.trim_start_matches('-'), radix).unwrap_or(0) & self.mask();
            let v = self.sign_extend(v);
            return if neg { -(v as f64) } else { v as f64 };
        }
        self.entry.replace(',', "").parse().unwrap_or(0.0)
    }

    fn sign_extend(&self, v: u64) -> i64 {
        let b = self.bits();
        if b == 64 { v as i64 } else if v & (1 << (b - 1)) != 0 { (v | !self.mask()) as i64 } else { v as i64 }
    }

    fn fmt(&self, v: f64) -> String {
        if self.mode == Mode::Programmer {
            let u = (v as i64 as u64) & self.mask();
            return match self.base {
                Base::Hex => format!("{u:X}"), Base::Oct => format!("{u:o}"), Base::Bin => format!("{u:b}"),
                Base::Dec => self.sign_extend(u).to_string(),
            };
        }
        if !v.is_finite() { return "Invalid input".into(); }
        if v == 0.0 { return "0".into(); }
        let a = v.abs();
        if !(1e-15..1e16).contains(&a) {
            // scientific notation like Win7: 1.2345e+20
            let s = format!("{v:.15e}");
            let (m, e) = s.split_once('e').unwrap();
            let m = m.trim_end_matches('0').trim_end_matches('.');
            let e: i32 = e.parse().unwrap_or(0);
            return format!("{m}e{}{}", if e >= 0 { "+" } else { "-" }, e.abs());
        }
        let s = format!("{:.*}", (15 - a.log10().floor().max(0.0) as usize).min(15), v);
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
        s
    }

    fn grouped(&self, s: &str) -> String {
        if !self.grouping || s.contains('e') { return s.to_string(); }
        let (neg, s) = s.strip_prefix('-').map(|r| (true, r)).unwrap_or((false, s));
        let (int, frac) = s.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((s, None));
        let n = if self.mode == Mode::Programmer && self.base != Base::Dec { 4 } else { 3 };
        let sep = if self.mode == Mode::Programmer && self.base != Base::Dec { ' ' } else { ',' };
        let mut out = String::new();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % n == 0 { out.push(sep); }
            out.push(c);
        }
        format!("{}{}{}", if neg { "-" } else { "" }, out, frac.map(|f| format!(".{f}")).unwrap_or_default())
    }

    fn set_value(&mut self, v: f64) {
        if !v.is_finite() { self.error(if v.is_nan() { "Invalid input" } else { "Overflow" }); return; }
        self.entry = self.fmt(v);
        self.fresh = true;
    }

    fn error(&mut self, msg: &str) {
        self.entry = msg.into();
        self.error = true;
        self.toks.clear();
        self.expr_text.clear();
        self.fresh = true;
    }

    fn digit(&mut self, d: char) {
        if self.error { self.clear(); }
        if self.fresh { self.entry = "0".into(); self.fresh = false; self.pending_unary = None; }
        if d == '.' {
            if self.mode == Mode::Programmer || self.entry.contains('.') { return; }
            self.entry.push('.');
            return;
        }
        let max = if self.mode == Mode::Programmer { 64 } else { 32 };
        if self.entry.trim_start_matches('-').replace('.', "").len() >= max { return; }
        if self.entry == "0" { self.entry = d.to_string(); } else if self.entry == "-0" { self.entry = format!("-{d}"); } else { self.entry.push(d); }
    }

    fn backspace(&mut self) {
        if self.fresh || self.error { return; }
        self.entry.pop();
        if self.entry.is_empty() || self.entry == "-" { self.entry = "0".into(); }
    }

    fn clear_entry(&mut self) { self.entry = "0".into(); self.fresh = true; self.error = false; self.pending_unary = None; }
    fn clear(&mut self) { self.clear_entry(); self.toks.clear(); self.expr_text.clear(); self.last = None; }

    fn negate(&mut self) {
        if self.error { return; }
        if !self.fresh {
            if let Some(r) = self.entry.strip_prefix('-') { self.entry = r.to_string(); } else if self.entry != "0" { self.entry.insert(0, '-'); }
        } else {
            let v = -self.value();
            self.unary_show("negate", v);
        }
    }

    /// Apply a unary function and show it in the expression line as Win7 does.
    fn unary_show(&mut self, name: &str, v: f64) {
        let arg = self.pending_unary.take().unwrap_or_else(|| self.entry.clone());
        let shown = format!("{name}({arg})");
        self.set_value(v);
        self.pending_unary = Some(shown);
    }

    fn unary(&mut self, f: &str) {
        if self.error { return; }
        let x = self.value();
        let to_rad = |a: f64, m: u8| match m { 0 => a.to_radians(), 2 => a * std::f64::consts::PI / 200.0, _ => a };
        let from_rad = |a: f64, m: u8| match m { 0 => a.to_degrees(), 2 => a * 200.0 / std::f64::consts::PI, _ => a };
        let inv = self.inv;
        let (name, v) = match f {
            "sqrt" => ("sqrt", if x < 0.0 { f64::NAN } else { x.sqrt() }),
            "recip" => ("reciproc", if x == 0.0 { self.error("Cannot divide by zero"); return; } else { 1.0 / x }),
            "pct" => {
                // x% of the running value (Win7 semantics)
                let base = self.eval_partial().unwrap_or(0.0);
                ("", base * x / 100.0)
            }
            "sq" => ("sqr", x * x),
            "cube" => ("cube", x * x * x),
            "cbrt" => ("cuberoot", x.cbrt()),
            "fact" => ("fact", if x < 0.0 || x > 170.0 || x.fract() != 0.0 { gamma_fact(x) } else { (1..=x as u64).map(|i| i as f64).product() }),
            "sin" => if inv { ("asind", from_rad(x.asin(), self.angle)) } else { ("sind", to_rad(x, self.angle).sin()) },
            "cos" => if inv { ("acosd", from_rad(x.acos(), self.angle)) } else { ("cosd", to_rad(x, self.angle).cos()) },
            "tan" => if inv { ("atand", from_rad(x.atan(), self.angle)) } else { ("tand", to_rad(x, self.angle).tan()) },
            "sinh" => if inv { ("asinh", x.asinh()) } else { ("sinh", x.sinh()) },
            "cosh" => if inv { ("acosh", x.acosh()) } else { ("cosh", x.cosh()) },
            "tanh" => if inv { ("atanh", x.atanh()) } else { ("tanh", x.tanh()) },
            "ln" => if inv { ("powe", x.exp()) } else { ("ln", x.ln()) },
            "log" => ("log", x.log10()),
            "tenx" => ("powten", 10f64.powf(x)),
            "int" => if inv { ("frac", x.fract()) } else { ("int", x.trunc()) },
            "dms" => if inv { ("degrees", dms_to_deg(x)) } else { ("dms", deg_to_dms(x)) },
            "not" => ("NOT", !(x as i64) as f64),
            "rol" => { let b = self.bits(); let u = (x as i64 as u64) & self.mask(); ("RoL", (((u << 1) | (u >> (b - 1))) & self.mask()) as i64 as f64) }
            "ror" => { let b = self.bits(); let u = (x as i64 as u64) & self.mask(); ("RoR", (((u >> 1) | (u << (b - 1))) & self.mask()) as i64 as f64) }
            "pi" => { self.entry = self.fmt(std::f64::consts::PI); self.fresh = true; return; }
            _ => return,
        };
        let v = if f == "sin" || f == "cos" || f == "tan" { (v * 1e15).round() / 1e15 } else { v };
        if name.is_empty() { self.set_value(v); } else { self.unary_show(name, v); }
        if inv { self.inv = false; }
    }

    fn push_entry(&mut self) {
        let v = self.value();
        let shown = self.pending_unary.take().unwrap_or_else(|| self.grouped(&self.entry));
        self.toks.push(Tok::Num(v));
        self.expr_text.push_str(&shown);
        self.expr_text.push(' ');
    }

    fn binop(&mut self, op: Op) {
        if self.error { return; }
        if self.fresh && matches!(self.toks.last(), Some(Tok::Op(_))) && self.pending_unary.is_none() {
            // changing the operator (2 + then ×)
            self.toks.pop();
            let t = self.expr_text.trim_end().to_string();
            let cut = t.rfind(' ').map(|i| i + 1).unwrap_or(0);
            self.expr_text = t[..cut].to_string();
        } else if !matches!(self.toks.last(), Some(Tok::RParen)) {
            self.push_entry();
        }
        self.toks.push(Tok::Op(op));
        self.expr_text.push_str(op.sym());
        self.expr_text.push(' ');
        if let Some(v) = self.eval_partial() { self.set_value(v); }
        self.fresh = true;
    }

    fn paren(&mut self, open: bool) {
        if self.mode == Mode::Standard || self.error { return; }
        if open {
            self.toks.push(Tok::LParen);
            self.expr_text.push_str("( ");
            self.fresh = true;
        } else {
            let depth: i32 = self.toks.iter().map(|t| match t { Tok::LParen => 1, Tok::RParen => -1, _ => 0 }).sum();
            if depth <= 0 { return; }
            if !matches!(self.toks.last(), Some(Tok::RParen)) { self.push_entry(); }
            self.toks.push(Tok::RParen);
            self.expr_text.push_str(") ");
            if let Some(v) = eval(&self.toks, self.mode != Mode::Standard, self) { self.set_value(v); }
        }
    }

    /// Running value shown after pressing an operator.
    fn eval_partial(&self) -> Option<f64> {
        let mut t = self.toks.clone();
        while matches!(t.last(), Some(Tok::Op(_)) | Some(Tok::LParen)) { t.pop(); }
        let depth: i32 = t.iter().map(|t| match t { Tok::LParen => 1, Tok::RParen => -1, _ => 0 }).sum();
        if depth != 0 { return None; }
        eval(&t, self.mode != Mode::Standard, self)
    }

    fn equals(&mut self) {
        if self.error { self.clear(); return; }
        if self.toks.is_empty() {
            // repeat the last operation: 2 + 3 = = =
            if let Some((op, rhs)) = self.last {
                let v = apply(op, self.value(), rhs, self);
                self.set_value(v);
            }
            return;
        }
        if !matches!(self.toks.last(), Some(Tok::RParen)) { self.push_entry(); }
        let depth: i32 = self.toks.iter().map(|t| match t { Tok::LParen => 1, Tok::RParen => -1, _ => 0 }).sum();
        for _ in 0..depth.max(0) { self.toks.push(Tok::RParen); }
        // remember "op rhs" for repeated equals
        if let (Some(Tok::Num(r)), Some(Tok::Op(o))) = (self.toks.iter().rev().find(|t| matches!(t, Tok::Num(_))), self.toks.iter().rev().find(|t| matches!(t, Tok::Op(_)))) {
            self.last = Some((*o, *r));
        }
        let expr = self.expr_text.trim().to_string();
        match eval(&self.toks, self.mode != Mode::Standard, self) {
            Some(v) if v.is_finite() => {
                self.set_value(v);
                self.history.push(format!("{expr} =\n{}", self.entry));
            }
            Some(v) if v.is_infinite() => self.error("Cannot divide by zero"),
            _ => self.error("Invalid input"),
        }
        self.toks.clear();
        self.expr_text.clear();
    }

    fn mem(&mut self, k: &str) {
        let v = self.value();
        match k {
            "MC" => { self.memory = 0.0; self.has_mem = false; }
            "MR" => { self.entry = self.fmt(self.memory); self.fresh = true; }
            "MS" => { self.memory = v; self.has_mem = true; self.fresh = true; }
            "M+" => { self.memory += v; self.has_mem = true; self.fresh = true; }
            "M-" => { self.memory -= v; self.has_mem = true; self.fresh = true; }
            _ => {}
        }
    }

    fn set_base(&mut self, b: Base) {
        let v = self.value();
        self.base = b;
        self.entry = self.fmt(v);
        self.fresh = true;
    }
}

fn apply(op: Op, a: f64, b: f64, c: &Calc) -> f64 {
    let prog = c.mode == Mode::Programmer;
    let (ia, ib) = (a as i64, b as i64);
    let r = match op {
        Op::Add => a + b, Op::Sub => a - b, Op::Mul => a * b,
        Op::Div => if b == 0.0 { f64::INFINITY } else if prog { (ia / ib) as f64 } else { a / b },
        Op::Pow => a.powf(b), Op::Root => a.powf(1.0 / b),
        Op::Mod => if b == 0.0 { f64::NAN } else if prog { (ia % ib) as f64 } else { a % b },
        Op::And => (ia & ib) as f64, Op::Or => (ia | ib) as f64, Op::Xor => (ia ^ ib) as f64,
        Op::Lsh => ((ia as u64).wrapping_shl(ib as u32)) as i64 as f64, Op::Rsh => (ia >> (ib.clamp(0, 63))) as f64,
    };
    if prog { c.sign_extend((r as i64 as u64) & c.mask()) as f64 } else { r }
}

/// Evaluate tokens: with precedence (scientific/programmer) or strictly left-to-right (standard).
fn eval(toks: &[Tok], precedence: bool, c: &Calc) -> Option<f64> {
    let mut out: Vec<f64> = vec![];
    let mut ops: Vec<Tok> = vec![];
    fn reduce(out: &mut Vec<f64>, op: Op, c: &Calc) -> Option<()> {
        let b = out.pop()?;
        let a = out.pop()?;
        out.push(apply(op, a, b, c));
        Some(())
    }
    for t in toks {
        match t {
            Tok::Num(v) => out.push(*v),
            Tok::LParen => ops.push(Tok::LParen),
            Tok::RParen => {
                while let Some(Tok::Op(o)) = ops.last().cloned() { ops.pop(); reduce(&mut out, o, c)?; }
                if matches!(ops.last(), Some(Tok::LParen)) { ops.pop(); }
            }
            Tok::Op(op) => {
                while let Some(Tok::Op(o)) = ops.last().cloned() {
                    let p = if precedence { o.prec() >= op.prec() && !matches!(op, Op::Pow) } else { true };
                    if !p { break; }
                    ops.pop();
                    reduce(&mut out, o, c)?;
                }
                ops.push(Tok::Op(*op));
            }
        }
    }
    while let Some(t) = ops.pop() { if let Tok::Op(o) = t { reduce(&mut out, o, c)?; } }
    out.pop()
}

fn gamma_fact(x: f64) -> f64 {
    // Lanczos approximation of Γ(x+1)
    if x < -0.5 { return f64::NAN; }
    let g = 7.0;
    let co = [0.999_999_999_999_809_9, 676.520_368_121_885_1, -1_259.139_216_722_402_8, 771.323_428_777_653_1,
        -176.615_029_162_140_6, 12.507_343_278_686_905, -0.138_571_095_265_720_12, 9.984_369_578_019_572e-6, 1.505_632_735_149_311_6e-7];
    let z = x;
    let mut a = co[0];
    let t = z + g + 0.5;
    for (i, c) in co.iter().enumerate().skip(1) { a += c / (z + i as f64); }
    (2.0 * std::f64::consts::PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * a
}
fn deg_to_dms(x: f64) -> f64 { let d = x.trunc(); let m = ((x - d) * 60.0).trunc(); let s = ((x - d) * 60.0 - m) * 60.0; d + m / 100.0 + s / 10000.0 }
fn dms_to_deg(x: f64) -> f64 { let d = x.trunc(); let m = ((x - d) * 100.0).trunc(); let s = ((x - d) * 100.0 - m) * 100.0; d + m / 60.0 + s / 3600.0 }

// ------------------------------------------------------------------ UI
struct Ui {
    win: gtk::ApplicationWindow,
    c: RefCell<Calc>,
    disp: gtk::Label,
    expr: gtk::Label,
    mem: gtk::Label,
    body: gtk::Box,
    hist_box: gtk::Box,
    bits: gtk::Label,
    keys: RefCell<Vec<(String, gtk::Button)>>,
}

const CSS: &str = r#"
.calc-win { background-image: linear-gradient(to bottom, #e9eff7, #d9e4f1); }
.calc-display { background-image: linear-gradient(to bottom, #f0f6fd, #ffffff 40%); border: 1px solid #8e9fb5; border-radius: 3px; padding: 3px 6px; box-shadow: inset 0 1px 2px rgba(0,0,0,0.15); }
.calc-expr { color: #5b6170; font-size: 9pt; }
.calc-num { color: #000; font-size: 18pt; }
.calc-mem { color: #000; font-size: 9pt; }
button.ck { min-width: 32px; min-height: 25px; padding: 0 2px; border-radius: 3px; border: 1px solid #8a9ab1;
  background-image: linear-gradient(to bottom, #f6f8fb, #e6ebf3 49%, #dbe2ed 50%, #e5ebf3); box-shadow: inset 0 0 0 1px rgba(255,255,255,0.8); color: #000; }
button.ck:hover { background-image: linear-gradient(to bottom, #fff9e8, #fdeec6 49%, #fbdb8c 50%, #fde9a6); border-color: #c7a24e; }
button.ck:active { background-image: linear-gradient(to bottom, #f8d891, #f5c35b 49%, #eda92b 50%, #f2bf54); border-color: #a3792b; }
button.ck:disabled { background-image: linear-gradient(to bottom, #f4f5f7, #eceef1); border-color: #c5ccd6; color: #9da5af; }
button.ck.eq { min-height: 54px; }
button.ck.wide { min-width: 68px; }
.calc-panel { background-color: transparent; }
.calc-bits { font-family: "Consolas", monospace; font-size: 8pt; color: #2b3a4f; background-color: rgba(255,255,255,0.4); border: 1px solid #b6c3d4; border-radius: 3px; padding: 3px 6px; }
.calc-hist { background-color: #fff; border: 1px solid #8e9fb5; border-radius: 3px; }
"#;

impl Ui {
    fn refresh(&self) {
        let c = self.c.borrow();
        let txt = if c.error { c.entry.clone() } else { c.grouped(&c.entry) };
        self.disp.set_text(&txt);
        let mut e = c.expr_text.clone();
        if let Some(u) = &c.pending_unary { if !c.toks.is_empty() || !e.is_empty() { e.push_str(u); } else { e = u.clone(); } }
        self.expr.set_text(&e);
        self.mem.set_text(if c.has_mem { "M" } else { "" });
        if c.mode == Mode::Programmer {
            let u = (c.value() as i64 as u64) & c.mask();
            let mut s = String::new();
            for row in (0..2i32).rev() {
                for nib in (0..8i32).rev() {
                    let n = row * 8 + nib;
                    if (n as u32) * 4 >= c.bits() { s.push_str("     "); continue; }
                    let v = (u >> (n * 4)) & 0xf;
                    s.push_str(&format!("{v:04b} "));
                }
                s.push('\n');
                for nib in (0..8).rev() {
                    let n = row * 8 + nib;
                    s.push_str(&if nib % 2 == 1 || n == 0 { format!("{:<5}", if (n * 4 + 3) < 64 { (n * 4 + 3).to_string() } else { String::new() }) } else { "     ".into() });
                }
                if row == 1 { s.push('\n'); }
            }
            self.bits.set_text(s.trim_end());
        }
        // enable/disable digit keys by base
        let base = c.base;
        let prog = c.mode == Mode::Programmer;
        for (k, b) in self.keys.borrow().iter() {
            let ok = match k.as_str() {
                "A" | "B" | "C" | "D" | "E" | "F" => prog && base == Base::Hex,
                "2" | "3" | "4" | "5" | "6" | "7" => !prog || base != Base::Bin,
                "8" | "9" => !prog || matches!(base, Base::Hex | Base::Dec),
                "." => !prog,
                _ => true,
            };
            b.set_sensitive(ok);
        }
    }

    fn press(self: &Rc<Self>, k: &str) {
        {
            let mut c = self.c.borrow_mut();
            match k {
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "." | "A" | "B" | "C" | "D" | "E" | "F" => c.digit(k.chars().next().unwrap()),
                "back" => c.backspace(),
                "CE" => c.clear_entry(),
                "Cl" => c.clear(),
                "neg" => c.negate(),
                "+" => c.binop(Op::Add), "-" => c.binop(Op::Sub), "*" => c.binop(Op::Mul), "/" => c.binop(Op::Div),
                "xy" => c.binop(Op::Pow), "yroot" => c.binop(Op::Root), "Mod" => c.binop(Op::Mod),
                "And" => c.binop(Op::And), "Or" => c.binop(Op::Or), "Xor" => c.binop(Op::Xor), "Lsh" => c.binop(Op::Lsh), "Rsh" => c.binop(Op::Rsh),
                "(" => c.paren(true), ")" => c.paren(false),
                "=" => c.equals(),
                "MC" | "MR" | "MS" | "M+" | "M-" => c.mem(k),
                "Inv" => c.inv = !c.inv,
                "Exp" => { if c.fresh { c.entry = "0".into(); c.fresh = false; } if !c.entry.contains('e') { c.entry.push_str("e+"); } }
                f => c.unary(f),
            }
            if c.error { drop(c); sounds::play(sounds::Event::DefaultBeep); }
        }
        self.refresh();
        self.refresh_history();
    }

    fn refresh_history(&self) {
        for ch in self.hist_box.children() { self.hist_box.remove(&ch); }
        let c = self.c.borrow();
        for h in c.history.iter().rev().take(30) {
            let l = gtk::Label::new(Some(h));
            l.set_xalign(1.0);
            l.set_justify(gtk::Justification::Right);
            l.set_margin_end(6);
            l.set_selectable(true);
            self.hist_box.pack_start(&l, false, false, 3);
        }
        if c.history.is_empty() { self.hist_box.pack_start(&gtk::Label::new(Some("There's no history yet")), false, false, 6); }
        self.hist_box.show_all();
    }

    fn key(self: &Rc<Self>, label: &str, code: &str, tip: &str) -> gtk::Button {
        let b = gtk::Button::with_label(label);
        b.style_context().add_class("ck");
        b.set_focus_on_click(false);
        if !tip.is_empty() { b.set_tooltip_text(Some(tip)); }
        let (u, c) = (self.clone(), code.to_string());
        b.connect_clicked(move |_| u.press(&c));
        self.keys.borrow_mut().push((code.to_string(), b.clone()));
        b
    }

    fn build_mode(self: &Rc<Self>) {
        for ch in self.body.children() { self.body.remove(&ch); }
        self.keys.borrow_mut().clear();
        let mode = self.c.borrow().mode;
        let grid = gtk::Grid::new();
        grid.set_row_spacing(5);
        grid.set_column_spacing(5);
        let k = |l: &str, c: &str| self.key(l, c, "");
        // standard block (5 cols), placed at column offset `x`
        let std_block = |g: &gtk::Grid, x: i32, y: i32| {
            let rows: [[(&str, &str); 5]; 5] = [
                [("MC", "MC"), ("MR", "MR"), ("MS", "MS"), ("M+", "M+"), ("M-", "M-")],
                [("←", "back"), ("CE", "CE"), ("C", "Cl"), ("±", "neg"), ("√", "sqrt")],
                [("7", "7"), ("8", "8"), ("9", "9"), ("/", "/"), ("%", "pct")],
                [("4", "4"), ("5", "5"), ("6", "6"), ("*", "*"), ("1/x", "recip")],
                [("1", "1"), ("2", "2"), ("3", "3"), ("-", "-"), ("=", "=")],
            ];
            for (r, row) in rows.iter().enumerate() {
                for (col, (l, c)) in row.iter().enumerate() {
                    if *c == "=" { let b = k(l, c); b.style_context().add_class("eq"); g.attach(&b, x + col as i32, y + r as i32, 1, 2); continue; }
                    if mode == Mode::Programmer && matches!(*c, "sqrt" | "pct" | "recip") { let b = k(l, c); b.set_sensitive(false); g.attach(&b, x + col as i32, y + r as i32, 1, 1); continue; }
                    g.attach(&k(l, c), x + col as i32, y + r as i32, 1, 1);
                }
            }
            let zero = k("0", "0");
            zero.style_context().add_class("wide");
            g.attach(&zero, x, y + 5, 2, 1);
            g.attach(&k(".", "."), x + 2, y + 5, 1, 1);
            g.attach(&k("+", "+"), x + 3, y + 5, 1, 1);
        };
        match mode {
            Mode::Standard => std_block(&grid, 0, 0),
            Mode::Scientific => {
                let ang = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let deg = gtk::RadioButton::with_label("Degrees");
                let rad = gtk::RadioButton::with_label_from_widget(&deg, "Radians");
                let grad = gtk::RadioButton::with_label_from_widget(&deg, "Grads");
                for (i, r) in [&deg, &rad, &grad].iter().enumerate() {
                    if self.c.borrow().angle == i as u8 { r.set_active(true); }
                    let u = self.clone();
                    r.connect_toggled(move |r| if r.is_active() { u.c.borrow_mut().angle = i as u8; });
                    ang.pack_start(*r, false, false, 0);
                }
                let fr = gtk::Frame::new(None);
                fr.add(&ang);
                grid.attach(&fr, 0, 0, 5, 1);
                let sci: [[(&str, &str); 5]; 5] = [
                    [("Inv", "Inv"), ("ln", "ln"), ("(", "("), (")", ")"), ("", "")],
                    [("Int", "int"), ("sinh", "sinh"), ("sin", "sin"), ("x²", "sq"), ("n!", "fact")],
                    [("dms", "dms"), ("cosh", "cosh"), ("cos", "cos"), ("xʸ", "xy"), ("ʸ√x", "yroot")],
                    [("π", "pi"), ("tanh", "tanh"), ("tan", "tan"), ("x³", "cube"), ("∛x", "cbrt")],
                    [("F-E", "fe"), ("Exp", "Exp"), ("Mod", "Mod"), ("log", "log"), ("10ˣ", "tenx")],
                ];
                for (r, row) in sci.iter().enumerate() {
                    for (col, (l, c)) in row.iter().enumerate() {
                        if c.is_empty() { continue; }
                        grid.attach(&k(l, c), col as i32, r as i32 + 1, 1, 1);
                    }
                }
                std_block(&grid, 5, 1);
            }
            Mode::Programmer => {
                let opts = gtk::Box::new(gtk::Orientation::Vertical, 2);
                let mut first: Option<gtk::RadioButton> = None;
                for (l, b) in [("Hex", Base::Hex), ("Dec", Base::Dec), ("Oct", Base::Oct), ("Bin", Base::Bin)] {
                    let r = match &first { None => gtk::RadioButton::with_label(l), Some(f) => gtk::RadioButton::with_label_from_widget(f, l) };
                    if first.is_none() { first = Some(r.clone()); }
                    r.set_active(self.c.borrow().base == b);
                    let u = self.clone();
                    r.connect_toggled(move |r| if r.is_active() { u.c.borrow_mut().set_base(b); u.refresh(); });
                    opts.pack_start(&r, false, false, 0);
                }
                let fr = gtk::Frame::new(None);
                fr.add(&opts);
                grid.attach(&fr, 0, 0, 1, 4);
                let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
                let mut first: Option<gtk::RadioButton> = None;
                for (l, w) in [("Qword", Word::Qword), ("Dword", Word::Dword), ("Word", Word::Word), ("Byte", Word::Byte)] {
                    let r = match &first { None => gtk::RadioButton::with_label(l), Some(f) => gtk::RadioButton::with_label_from_widget(f, l) };
                    if first.is_none() { first = Some(r.clone()); }
                    r.set_active(self.c.borrow().word == w);
                    let u = self.clone();
                    r.connect_toggled(move |r| if r.is_active() { let mut c = u.c.borrow_mut(); let v = c.value(); c.word = w; c.entry = c.fmt(v); drop(c); u.refresh(); });
                    words.pack_start(&r, false, false, 0);
                }
                let fr = gtk::Frame::new(None);
                fr.add(&words);
                grid.attach(&fr, 0, 4, 1, 3);
                let prog: [[(&str, &str); 3]; 6] = [
                    [("", ""), ("Mod", "Mod"), ("A", "A")], [("(", "("), (")", ")"), ("B", "B")],
                    [("RoL", "rol"), ("RoR", "ror"), ("C", "C")], [("Or", "Or"), ("Xor", "Xor"), ("D", "D")],
                    [("Lsh", "Lsh"), ("Rsh", "Rsh"), ("E", "E")], [("Not", "not"), ("And", "And"), ("F", "F")],
                ];
                for (r, row) in prog.iter().enumerate() {
                    for (col, (l, c)) in row.iter().enumerate() {
                        if c.is_empty() { continue; }
                        grid.attach(&k(l, c), col as i32 + 1, r as i32, 1, 1);
                    }
                }
                std_block(&grid, 4, 0);
            }
        }
        self.bits.set_visible(mode == Mode::Programmer);
        self.body.pack_start(&grid, false, false, 0);
        self.body.show_all();
        self.refresh();
        let _ = config::conf_dir();
        // shrink to fit the new mode
        self.win.resize(1, 1);
    }

    fn set_mode(self: &Rc<Self>, m: Mode) {
        {
            let mut c = self.c.borrow_mut();
            if c.mode == m { return; }
            let v = c.value();
            c.mode = m;
            c.toks.clear();
            c.expr_text.clear();
            c.entry = c.fmt(if m == Mode::Programmer { v.trunc() } else { v });
            c.fresh = true;
        }
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("calc-mode"), match m { Mode::Standard => "std", Mode::Scientific => "sci", Mode::Programmer => "prog" });
        self.build_mode();
    }
}

fn build(app: &gtk::Application) {
    theme::init_app();
    theme::load_css(CSS);
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Calculator");
    win.set_resizable(false);
    win.set_icon_name(Some(util::first_icon(&["accessories-calculator"])));
    win.style_context().add_class("calc-win");
    let accel = gtk::AccelGroup::new();
    win.add_accel_group(&accel);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    root.pack_start(&menubar, false, false, 0);
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    outer.set_border_width(10);
    let main = gtk::Box::new(gtk::Orientation::Vertical, 8);
    // display
    let dbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dbox.style_context().add_class("calc-display");
    let expr = gtk::Label::new(None);
    expr.set_xalign(1.0);
    expr.style_context().add_class("calc-expr");
    expr.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let mem = gtk::Label::new(None);
    mem.style_context().add_class("calc-mem");
    mem.set_valign(gtk::Align::End);
    let disp = gtk::Label::new(Some("0"));
    disp.set_xalign(1.0);
    disp.style_context().add_class("calc-num");
    row.pack_start(&mem, false, false, 0);
    row.pack_start(&disp, true, true, 0);
    dbox.pack_start(&expr, false, false, 0);
    dbox.pack_start(&row, false, false, 0);
    main.pack_start(&dbox, false, false, 0);
    let bits = gtk::Label::new(None);
    bits.style_context().add_class("calc-bits");
    bits.set_xalign(0.0);
    main.pack_start(&bits, false, false, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.pack_start(&body, false, false, 0);
    outer.pack_start(&main, false, false, 0);
    // history panel (Ctrl+H)
    let hist_sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    hist_sw.style_context().add_class("calc-hist");
    hist_sw.set_size_request(180, -1);
    hist_sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let hist_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    hist_sw.add(&hist_box);
    outer.pack_start(&hist_sw, false, false, 0);
    root.pack_start(&outer, true, true, 0);
    win.add(&root);
    let ui = Rc::new(Ui { win: win.clone(), c: RefCell::new(Calc::new()), disp, expr, mem, body, hist_box, bits, keys: RefCell::new(vec![]) });

    let item = |m: &gtk::Menu, l: &str, acc: &str, f: Box<dyn Fn()>| {
        let it = gtk::MenuItem::with_mnemonic(l);
        if !acc.is_empty() { let (k, md) = gtk::accelerator_parse(acc); it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE); }
        it.connect_activate(move |_| f());
        m.append(&it);
    };
    // View
    let m = gtk::Menu::new();
    let mut group: Option<gtk::RadioMenuItem> = None;
    let saved = std::fs::read_to_string(config::conf_dir().join("calc-mode")).unwrap_or_default();
    for (l, acc, mode) in [("_Standard", "<Alt>1", Mode::Standard), ("S_cientific", "<Alt>2", Mode::Scientific), ("_Programmer", "<Alt>3", Mode::Programmer)] {
        let it = match &group { None => gtk::RadioMenuItem::with_mnemonic(l), Some(g) => gtk::RadioMenuItem::with_mnemonic_from_widget(g, Some(l)) };
        if group.is_none() { group = Some(it.clone()); }
        let (k, md) = gtk::accelerator_parse(acc);
        it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE);
        it.set_active(match saved.as_str() { "sci" => mode == Mode::Scientific, "prog" => mode == Mode::Programmer, _ => mode == Mode::Standard });
        let u = ui.clone();
        it.connect_toggled(move |i| if i.is_active() { u.set_mode(mode) });
        m.append(&it);
    }
    m.append(&gtk::SeparatorMenuItem::new());
    let hist = gtk::CheckMenuItem::with_mnemonic("_History");
    let (k, md) = gtk::accelerator_parse("<Control>h");
    hist.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE);
    { let (sw, w) = (hist_sw.clone(), win.clone()); hist.connect_toggled(move |h| { sw.set_visible(h.is_active()); w.resize(1, 1); }); }
    m.append(&hist);
    let grp = gtk::CheckMenuItem::with_mnemonic("Dig_it grouping");
    { let u = ui.clone(); grp.connect_toggled(move |g| { u.c.borrow_mut().grouping = g.is_active(); u.refresh(); }); }
    m.append(&grp);
    let top = gtk::MenuItem::with_mnemonic("_View"); top.set_submenu(Some(&m)); menubar.append(&top);
    // Edit
    let m = gtk::Menu::new();
    { let u = ui.clone(); item(&m, "_Copy", "<Control>c", Box::new(move || {
        let t = u.c.borrow().entry.clone();
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&t);
    })); }
    { let u = ui.clone(); item(&m, "_Paste", "<Control>v", Box::new(move || {
        let u2 = u.clone();
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).request_text(move |_, t| {
            if let Some(t) = t { for ch in t.chars() { let s = ch.to_string(); if "0123456789.+-*/=()".contains(ch) { u2.press(&s); } } }
        });
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Edit"); top.set_submenu(Some(&m)); menubar.append(&top);
    // Help
    let m = gtk::Menu::new();
    { let w = win.clone(); item(&m, "_About Calculator", "", Box::new(move || {
        let d = gtk::AboutDialog::new();
        d.set_transient_for(Some(&w));
        d.set_program_name("Calculator");
        d.set_comments(Some("Aero7 Calculator – a Windows 7 style calculator for Linux"));
        d.set_logo_icon_name(Some(util::first_icon(&["accessories-calculator"])));
        d.run();
        d.close();
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);

    // keyboard, like Win7
    {
        let u = ui.clone();
        win.connect_key_press_event(move |_, e| {
            use gdk::keys::constants as K;
            let kv = e.keyval();
            let ctrl = e.state().contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl { return glib::Propagation::Proceed; }
            let code: Option<String> = match kv {
                k if k == K::Return || k == K::KP_Enter || k == K::equal => Some("=".into()),
                k if k == K::BackSpace => Some("back".into()),
                k if k == K::Escape => Some("Cl".into()),
                k if k == K::Delete => Some("CE".into()),
                k if k == K::F9 => Some("neg".into()),
                k if k == K::KP_Add || k == K::plus => Some("+".into()),
                k if k == K::KP_Subtract || k == K::minus => Some("-".into()),
                k if k == K::KP_Multiply || k == K::asterisk => Some("*".into()),
                k if k == K::KP_Divide || k == K::slash => Some("/".into()),
                k if k == K::percent => Some("pct".into()),
                k if k == K::at => Some("sqrt".into()),
                k if k == K::r || k == K::R => Some("recip".into()),
                k if k == K::period || k == K::comma || k == K::KP_Decimal => Some(".".into()),
                k if k == K::parenleft => Some("(".into()),
                k if k == K::parenright => Some(")".into()),
                _ => kv.to_unicode().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_uppercase().to_string()),
            };
            if let Some(c) = code {
                if c.len() == 1 && c.chars().next().unwrap().is_ascii_alphabetic() && u.c.borrow().base != Base::Hex { return glib::Propagation::Proceed; }
                u.press(&c);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    root.show_all();
    hist_sw.hide();
    {
        let mut c = ui.c.borrow_mut();
        c.mode = match saved.as_str() { "sci" => Mode::Scientific, "prog" => Mode::Programmer, _ => Mode::Standard };
    }
    ui.build_mode();
    ui.refresh_history();
    win.show();
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::new(Some("org.aero7.Calculator"), gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(build);
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(keys: &[&str], mode: Mode) -> String {
        let mut c = Calc::new();
        c.mode = mode;
        for k in keys {
            match *k {
                "+" => c.binop(Op::Add), "-" => c.binop(Op::Sub), "*" => c.binop(Op::Mul), "/" => c.binop(Op::Div),
                "=" => c.equals(), "(" => c.paren(true), ")" => c.paren(false),
                d => for ch in d.chars() { c.digit(ch) },
            }
        }
        c.entry
    }
    #[test] fn standard_is_left_to_right() { assert_eq!(run(&["2", "+", "3", "*", "4", "="], Mode::Standard), "20"); }
    #[test] fn scientific_precedence() { assert_eq!(run(&["2", "+", "3", "*", "4", "="], Mode::Scientific), "14"); }
    #[test] fn parens() { assert_eq!(run(&["(", "2", "+", "3", ")", "*", "4", "="], Mode::Scientific), "20"); }
    #[test] fn repeat_equals() { assert_eq!(run(&["2", "+", "3", "=", "=", "="], Mode::Standard), "11"); }
    #[test] fn decimals() { assert_eq!(run(&["0.1", "+", "0.2", "="], Mode::Standard), "0.3"); }
    #[test] fn div_zero() { assert_eq!(run(&["5", "/", "0", "="], Mode::Standard), "Cannot divide by zero"); }
}
