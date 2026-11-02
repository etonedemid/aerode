//! Calculator – Windows 7 Calculator remake: Standard / Scientific / Programmer / Statistics modes,
//! history, memory, digit grouping, and the Unit conversion, Date calculation and Worksheets panels.
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aerode::{config, sounds, theme, util};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Standard, Scientific, Programmer, Statistics }

impl Mode {
    fn key(self) -> &'static str { match self { Mode::Standard => "std", Mode::Scientific => "sci", Mode::Programmer => "prog", Mode::Statistics => "stat" } }
}

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

impl Base { fn radix(self) -> u32 { match self { Base::Hex => 16, Base::Dec => 10, Base::Oct => 8, Base::Bin => 2 } } }

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
    fe: bool,             // F-E: scientific notation
    /// (mode, expression, result): Win7 keeps a history per mode (none in Programmer)
    history: Vec<(Mode, String, String)>,
    pending_unary: Option<String>,
    stats: Vec<f64>,
}

impl Calc {
    fn new() -> Calc {
        Calc { mode: Mode::Standard, entry: "0".into(), fresh: true, toks: vec![], expr_text: String::new(), last: None,
            memory: 0.0, has_mem: false, error: false, inv: false, angle: 0, base: Base::Dec, word: Word::Qword,
            grouping: false, fe: false, history: vec![], pending_unary: None, stats: vec![] }
    }

    fn mask(&self) -> u64 { match self.word { Word::Qword => u64::MAX, Word::Dword => 0xffff_ffff, Word::Word => 0xffff, Word::Byte => 0xff } }
    fn bits(&self) -> u32 { match self.word { Word::Qword => 64, Word::Dword => 32, Word::Word => 16, Word::Byte => 8 } }

    fn value(&self) -> f64 {
        if self.mode == Mode::Programmer {
            let t = self.entry.replace([' ', ','], "");
            let neg = t.starts_with('-');
            let v = u64::from_str_radix(t.trim_start_matches('-'), self.base.radix()).unwrap_or(0) & self.mask();
            let v = self.sign_extend(v);
            return if neg { -(v as f64) } else { v as f64 };
        }
        let t = self.entry.replace(',', "");
        let t = t.trim_end_matches(['+', '-']).trim_end_matches('e');
        t.parse().unwrap_or(0.0)
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
        fmt_num(v, self.fe)
    }

    fn grouped(&self, s: &str) -> String {
        if !self.grouping || s.contains('e') { return s.to_string(); }
        let (neg, s) = s.strip_prefix('-').map(|r| (true, r)).unwrap_or((false, s));
        let (int, frac) = s.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((s, None));
        let prog_nondec = self.mode == Mode::Programmer && self.base != Base::Dec;
        let n = if prog_nondec { 4 } else { 3 };
        let sep = if prog_nondec { ' ' } else { ',' };
        let mut out = String::new();
        let len = int.chars().count();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (len - i) % n == 0 { out.push(sep); }
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
            if self.mode == Mode::Programmer || self.entry.contains('.') || self.entry.contains('e') { return; }
            self.entry.push('.');
            return;
        }
        if self.mode == Mode::Programmer {
            // as many digits as the word size holds
            let t = self.entry.trim_start_matches('-');
            let cand = if t == "0" { d.to_string() } else { format!("{t}{d}") };
            match u128::from_str_radix(&cand, self.base.radix()) { Ok(v) if v <= self.mask() as u128 => {}, _ => return }
        } else if self.entry.trim_start_matches('-').replace(['.', ','], "").len() >= 32 { return; }
        if self.entry == "0" { self.entry = d.to_string(); } else if self.entry == "-0" { self.entry = format!("-{d}"); } else { self.entry.push(d); }
    }

    fn backspace(&mut self) {
        if self.fresh || self.error { return; }
        self.entry.pop();
        if self.entry.ends_with("e+") || self.entry.ends_with("e-") { self.entry.truncate(self.entry.len() - 2); }
        if self.entry.is_empty() || self.entry == "-" { self.entry = "0".into(); }
    }

    fn clear_entry(&mut self) { self.entry = "0".into(); self.fresh = true; self.error = false; self.pending_unary = None; }
    fn clear(&mut self) { self.clear_entry(); self.toks.clear(); self.expr_text.clear(); self.last = None; }

    fn negate(&mut self) {
        if self.error { return; }
        if !self.fresh {
            if let Some(i) = self.entry.find("e+").or_else(|| self.entry.find("e-")) {
                // ± while typing an exponent flips the exponent's sign
                let s = if &self.entry[i + 1..i + 2] == "+" { "-" } else { "+" };
                self.entry.replace_range(i + 1..i + 2, s);
            } else if let Some(r) = self.entry.strip_prefix('-') { self.entry = r.to_string(); } else if self.entry != "0" { self.entry.insert(0, '-'); }
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
        if !self.error { self.pending_unary = Some(shown); }
    }

    fn unary(&mut self, f: &str) {
        if self.error { return; }
        let x = self.value();
        let to_rad = |a: f64, m: u8| match m { 0 => a.to_radians(), 2 => a * std::f64::consts::PI / 200.0, _ => a };
        let from_rad = |a: f64, m: u8| match m { 0 => a.to_degrees(), 2 => a * 200.0 / std::f64::consts::PI, _ => a };
        let inv = self.inv;
        let unit = ["d", "r", "g"][self.angle as usize % 3];
        let (name, v): (String, f64) = match f {
            "sqrt" => ("sqrt".into(), if x < 0.0 { f64::NAN } else { x.sqrt() }),
            "recip" => ("reciproc".into(), if x == 0.0 { self.error("Cannot divide by zero"); return; } else { 1.0 / x }),
            "pct" => {
                // x% of the running value (Win7 semantics)
                let base = self.eval_partial().unwrap_or(0.0);
                (String::new(), base * x / 100.0)
            }
            "sq" => if inv { ("sqrt".into(), if x < 0.0 { f64::NAN } else { x.sqrt() }) } else { ("sqr".into(), x * x) },
            "cube" => if inv { ("cuberoot".into(), x.cbrt()) } else { ("cube".into(), x * x * x) },
            "cbrt" => ("cuberoot".into(), x.cbrt()),
            "fact" => ("fact".into(), if x < 0.0 || x > 170.0 || x.fract() != 0.0 { gamma_fact(x) } else { (1..=x as u64).map(|i| i as f64).product() }),
            "sin" => if inv { (format!("asin{unit}"), from_rad(x.asin(), self.angle)) } else { (format!("sin{unit}"), to_rad(x, self.angle).sin()) },
            "cos" => if inv { (format!("acos{unit}"), from_rad(x.acos(), self.angle)) } else { (format!("cos{unit}"), to_rad(x, self.angle).cos()) },
            "tan" => if inv { (format!("atan{unit}"), from_rad(x.atan(), self.angle)) } else { (format!("tan{unit}"), to_rad(x, self.angle).tan()) },
            "sinh" => if inv { ("asinh".into(), x.asinh()) } else { ("sinh".into(), x.sinh()) },
            "cosh" => if inv { ("acosh".into(), x.acosh()) } else { ("cosh".into(), x.cosh()) },
            "tanh" => if inv { ("atanh".into(), x.atanh()) } else { ("tanh".into(), x.tanh()) },
            "ln" => if inv { ("powe".into(), x.exp()) } else { ("ln".into(), if x <= 0.0 { f64::NAN } else { x.ln() }) },
            "log" => ("log".into(), if x <= 0.0 { f64::NAN } else { x.log10() }),
            "tenx" => ("powten".into(), 10f64.powf(x)),
            "int" => if inv { ("frac".into(), x.fract()) } else { ("int".into(), x.trunc()) },
            "dms" => if inv { ("degrees".into(), dms_to_deg(x)) } else { ("dms".into(), deg_to_dms(x)) },
            "not" => ("NOT".into(), !(x as i64) as f64),
            "rol" => { let b = self.bits(); let u = (x as i64 as u64) & self.mask(); ("RoL".into(), self.sign_extend(((u << 1) | (u >> (b - 1))) & self.mask()) as f64) }
            "ror" => { let b = self.bits(); let u = (x as i64 as u64) & self.mask(); ("RoR".into(), self.sign_extend(((u >> 1) | (u << (b - 1))) & self.mask()) as f64) }
            "pi" => { self.entry = self.fmt(if inv { 2.0 * std::f64::consts::PI } else { std::f64::consts::PI }); self.fresh = true; self.inv = false; return; }
            _ => return,
        };
        let v = if ["sin", "cos", "tan"].contains(&f) && !inv { (v * 1e15).round() / 1e15 } else { v };
        let v = if self.mode == Mode::Programmer { self.sign_extend((v as i64 as u64) & self.mask()) as f64 } else { v };
        if name.is_empty() { self.set_value(v); } else { self.unary_show(&name, v); }
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
        if matches!(self.mode, Mode::Standard | Mode::Statistics) || self.error { return; }
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

    fn open_parens(&self) -> i32 { self.toks.iter().map(|t| match t { Tok::LParen => 1, Tok::RParen => -1, _ => 0 }).sum() }

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
            self.pending_unary = None;
            return;
        }
        if !matches!(self.toks.last(), Some(Tok::RParen)) { self.push_entry(); }
        let depth = self.open_parens();
        for _ in 0..depth.max(0) { self.toks.push(Tok::RParen); self.expr_text.push_str(") "); }
        // remember "op rhs" for repeated equals
        if let (Some(Tok::Num(r)), Some(Tok::Op(o))) = (self.toks.iter().rev().find(|t| matches!(t, Tok::Num(_))), self.toks.iter().rev().find(|t| matches!(t, Tok::Op(_)))) {
            self.last = Some((*o, *r));
        }
        let expr = self.expr_text.trim().to_string();
        match eval(&self.toks, self.mode != Mode::Standard, self) {
            Some(v) if v.is_finite() => {
                self.set_value(v);
                if self.mode != Mode::Programmer { self.history.push((self.mode, expr, self.entry.clone())); }
            }
            Some(v) if v.is_infinite() => self.error("Cannot divide by zero"),
            _ => self.error("Invalid input"),
        }
        self.toks.clear();
        self.expr_text.clear();
    }

    fn mem(&mut self, k: &str) {
        if self.error && k != "MC" && k != "MR" { return; }
        let v = self.value();
        match k {
            "MC" => { self.memory = 0.0; self.has_mem = false; }
            "MR" => { if self.error { self.clear(); } self.entry = self.fmt(self.memory); self.fresh = true; self.pending_unary = None; }
            "MS" => { self.memory = v; self.has_mem = true; self.fresh = true; }
            "M+" => { self.memory += v; self.has_mem = true; self.fresh = true; }
            "M-" => { self.memory -= v; self.has_mem = true; self.fresh = true; }
            _ => {}
        }
    }

    fn set_base(&mut self, b: Base) {
        let v = self.value();
        self.base = b;
        if !self.error { self.entry = self.fmt(v); }
        self.fresh = true;
    }

    // ---- Statistics mode
    fn stat_add(&mut self) {
        if self.error { return; }
        let v = self.value();
        self.stats.push(v);
        self.entry = "0".into();
        self.fresh = true;
        self.pending_unary = None;
    }

    fn stat(&mut self, f: &str) {
        if self.error { self.clear(); }
        let n = self.stats.len() as f64;
        let sum: f64 = self.stats.iter().sum();
        let sumsq: f64 = self.stats.iter().map(|x| x * x).sum();
        let (name, v) = match f {
            "avg" => ("x̄", sum / n),
            "avgsq" => ("x̄²", sumsq / n),
            "sum" => ("Σx", sum),
            "sumsq" => ("Σx²", sumsq),
            "sdn" => ("σn", (sumsq / n - (sum / n).powi(2)).max(0.0).sqrt()),
            "sdn1" => ("σn-1", ((sumsq - sum * sum / n) / (n - 1.0)).max(0.0).sqrt()),
            _ => return,
        };
        if n == 0.0 || (f == "sdn1" && n < 2.0) { self.error("Invalid input"); return; }
        self.set_value(v);
        self.history.push((Mode::Statistics, format!("{name}"), self.entry.clone()));
    }
}

/// Win7 shows up to 16 significant digits, switching to scientific notation for big/small numbers.
fn fmt_num(v: f64, fe: bool) -> String {
    if !v.is_finite() { return "Invalid input".into(); }
    if v == 0.0 { return if fe { "0.e+0".into() } else { "0".into() }; }
    let a = v.abs();
    if fe || !(1e-15..1e16).contains(&a) {
        // scientific notation like Win7: 1.2345e+20
        let s = format!("{v:.15e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0');
        let m = if m.ends_with('.') { m.to_string() } else if m.contains('.') { m.to_string() } else { format!("{m}.") };
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{m}e{}{}", if e >= 0 { "+" } else { "-" }, e.abs());
    }
    let s = format!("{:.*}", (15 - a.log10().floor().max(0.0) as usize).min(15), v);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn apply(op: Op, a: f64, b: f64, c: &Calc) -> f64 {
    let prog = c.mode == Mode::Programmer;
    let (ia, ib) = (a as i64, b as i64);
    let r = match op {
        Op::Add => a + b, Op::Sub => a - b, Op::Mul => a * b,
        Op::Div => if b == 0.0 { f64::INFINITY } else if prog { ia.wrapping_div(ib) as f64 } else { a / b },
        Op::Pow => a.powf(b), Op::Root => a.powf(1.0 / b),
        Op::Mod => if b == 0.0 { f64::NAN } else if prog { ia.wrapping_rem(ib) as f64 } else { a % b },
        Op::And => (ia & ib) as f64, Op::Or => (ia | ib) as f64, Op::Xor => (ia ^ ib) as f64,
        Op::Lsh => ((ia as u64).wrapping_shl(ib as u32)) as i64 as f64, Op::Rsh => (ia >> (ib.clamp(0, 63))) as f64,
    };
    if prog && r.is_finite() { c.sign_extend((r as i64 as u64) & c.mask()) as f64 } else { r }
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

// ------------------------------------------------------------------ unit conversion
/// Win7's unit lists; factors convert to the SI unit of the category.
const UNITS: &[(&str, &[(&str, f64)])] = &[
    ("Angle", &[("Degree", std::f64::consts::PI / 180.0), ("Gradian", std::f64::consts::PI / 200.0), ("Radian", 1.0)]),
    ("Area", &[("Acres", 4046.8564224), ("Hectares", 10000.0), ("Square centimeter", 1e-4), ("Square feet", 0.09290304), ("Square inch", 0.00064516),
        ("Square kilometer", 1e6), ("Square meter", 1.0), ("Square mile", 2589988.110336), ("Square millimeter", 1e-6), ("Square yard", 0.83612736)]),
    ("Energy", &[("British Thermal Unit", 1055.05585262), ("Calorie", 4.1868), ("Electron-volts", 1.602176565e-19), ("Foot-pound", 1.3558179483314),
        ("Joule", 1.0), ("Kilocalorie", 4186.8), ("Kilojoule", 1000.0)]),
    ("Length", &[("Angstroms", 1e-10), ("Centimeters", 0.01), ("Chains", 20.1168), ("Fathoms", 1.8288), ("Feet", 0.3048), ("Hands", 0.1016), ("Inches", 0.0254),
        ("Kilometers", 1000.0), ("Links", 0.201168), ("Meters", 1.0), ("Microns", 1e-6), ("Miles", 1609.344), ("Millimeters", 0.001), ("Nanometers", 1e-9),
        ("Nautical miles", 1852.0), ("Picas", 0.0042333333333), ("Rods", 5.0292), ("Spans", 0.2286), ("Yards", 0.9144)]),
    ("Power", &[("BTU/minute", 17.584264), ("Foot-pound/minute", 0.0225969658), ("Horsepower", 745.69987158), ("Kilowatt", 1000.0), ("Watt", 1.0)]),
    ("Pressure", &[("Atmosphere", 101325.0), ("Bar", 100000.0), ("Kilo Pascal", 1000.0), ("Millimeter of mercury", 133.322387415), ("Pascal", 1.0), ("Pound per square inch (PSI)", 6894.757293)]),
    ("Temperature", &[("Degrees Celsius", 0.0), ("Degrees Fahrenheit", 0.0), ("Kelvin", 0.0)]),
    ("Time", &[("Day", 86400.0), ("Hour", 3600.0), ("Microsecond", 1e-6), ("Millisecond", 0.001), ("Minute", 60.0), ("Second", 1.0), ("Week", 604800.0)]),
    ("Velocity", &[("Centimeters per second", 0.01), ("Feet per second", 0.3048), ("Kilometers per hour", 1.0 / 3.6), ("Knots", 0.514444444), ("Mach (at std. atm.)", 340.3),
        ("Meters per second", 1.0), ("Miles per hour", 0.44704)]),
    ("Volume", &[("Cubic centimeter", 1e-6), ("Cubic feet", 0.028316846592), ("Cubic inch", 1.6387064e-5), ("Cubic meter", 1.0), ("Cubic yard", 0.764554857984),
        ("Fluid ounce (UK)", 2.84130625e-5), ("Fluid ounce (US)", 2.95735295625e-5), ("Gallon (UK)", 0.00454609), ("Gallon (US)", 0.003785411784), ("Liter", 0.001),
        ("Pint (UK)", 0.00056826125), ("Pint (US)", 0.000473176473), ("Quart (UK)", 0.0011365225), ("Quart (US)", 0.000946352946)]),
    ("Weight / mass", &[("Carat", 0.0002), ("Centigram", 1e-5), ("Decigram", 1e-4), ("Dekagram", 0.01), ("Gram", 0.001), ("Hectogram", 0.1), ("Kilogram", 1.0),
        ("Long ton", 1016.0469088), ("Milligram", 1e-6), ("Ounce", 0.028349523125), ("Pound", 0.45359237), ("Short ton", 907.18474), ("Stone", 6.35029318), ("Tonne", 1000.0)]),
];

fn convert(cat: usize, from: usize, to: usize, v: f64) -> f64 {
    let (name, units) = UNITS[cat];
    if name == "Temperature" {
        let k = match from { 0 => v + 273.15, 1 => (v - 32.0) * 5.0 / 9.0 + 273.15, _ => v };
        return match to { 0 => k - 273.15, 1 => (k - 273.15) * 9.0 / 5.0 + 32.0, _ => k };
    }
    v * units[from].1 / units[to].1
}

// ------------------------------------------------------------------ date calculation
fn days_in_month(y: i32, m: u32) -> u32 {
    use chrono::NaiveDate;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1).and_then(|d| d.pred_opt()).map(|d| chrono::Datelike::day(&d)).unwrap_or(30)
}

/// "1 year, 2 months, 3 weeks, 4 days" between two dates (Win7's first result line).
fn date_diff(a: chrono::NaiveDate, b: chrono::NaiveDate) -> (String, i64) {
    use chrono::Datelike;
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let days_total = (b - a).num_days();
    if days_total == 0 { return ("Same dates".into(), 0); }
    let mut months = (b.year() - a.year()) * 12 + b.month() as i32 - a.month() as i32;
    if b.day() < a.day() { months -= 1; }
    let (y, m) = (months / 12, months % 12);
    // the date `months` months after a
    let tm = a.month0() as i32 + months;
    let ty = a.year() + tm.div_euclid(12);
    let tmo = tm.rem_euclid(12) as u32 + 1;
    let anchor = chrono::NaiveDate::from_ymd_opt(ty, tmo, a.day().min(days_in_month(ty, tmo))).unwrap_or(a);
    let rest = (b - anchor).num_days().max(0);
    let (w, d) = (rest / 7, rest % 7);
    let mut parts = vec![];
    let plural = |n: i64, s: &str| format!("{n} {s}{}", if n == 1 { "" } else { "s" });
    if y > 0 { parts.push(plural(y as i64, "year")); }
    if m > 0 { parts.push(plural(m as i64, "month")); }
    if w > 0 { parts.push(plural(w, "week")); }
    if d > 0 { parts.push(plural(d, "day")); }
    (parts.join(", "), days_total)
}

fn add_date(a: chrono::NaiveDate, years: i32, months: i32, days: i64, sub: bool) -> Option<chrono::NaiveDate> {
    use chrono::Datelike;
    let sign = if sub { -1 } else { 1 };
    let tm = a.month0() as i32 + sign * (years * 12 + months);
    let ty = a.year() + tm.div_euclid(12);
    let tmo = tm.rem_euclid(12) as u32 + 1;
    let d = chrono::NaiveDate::from_ymd_opt(ty, tmo, a.day().min(days_in_month(ty, tmo)))?;
    d.checked_add_signed(chrono::Duration::days(sign as i64 * days))
}

// ------------------------------------------------------------------ worksheets
const SHEETS: &[(&str, &[&str])] = &[
    ("Mortgage", &["Purchase price", "Down payment", "Term (years)", "Interest rate (%)", "Monthly payment"]),
    ("Vehicle lease", &["Lease value", "Residual value", "Term (years)", "Interest rate (%)", "Monthly payment"]),
    ("Fuel economy (mpg)", &["Distance (miles)", "Fuel used (gallons)", "Fuel economy (mpg)"]),
    ("Fuel economy (L/100 km)", &["Distance (kilometers)", "Fuel used (liters)", "Fuel economy (L/100 km)"]),
];

/// Solve worksheet `sheet` for field `want` given the other fields' values.
fn solve_sheet(sheet: usize, want: usize, v: &[f64]) -> Option<f64> {
    let annuity = |r: f64, n: f64| if r == 0.0 { n } else { (1.0 - (1.0 + r).powf(-n)) / r };
    let r = |pct: f64| pct / 1200.0;
    let res = match sheet {
        0 => {
            // [price, down, years, rate, payment]
            let (price, down, years, rate, pay) = (v[0], v[1], v[2], v[3], v[4]);
            let (i, n) = (r(rate), years * 12.0);
            match want {
                4 => (price - down) / annuity(i, n),
                0 => pay * annuity(i, n) + down,
                1 => price - pay * annuity(i, n),
                2 => bisect(|t| pay * annuity(i, t * 12.0) - (price - down), 0.01, 1000.0)?,
                _ => return None,
            }
        }
        1 => {
            // [value, residual, years, rate, payment]: payment pays off value - discounted residual
            let (val, res, years, rate, pay) = (v[0], v[1], v[2], v[3], v[4]);
            let (i, n) = (r(rate), years * 12.0);
            let disc = (1.0 + i).powf(-n);
            match want {
                4 => (val - res * disc) / annuity(i, n),
                0 => pay * annuity(i, n) + res * disc,
                1 => (val - pay * annuity(i, n)) / disc,
                2 => bisect(|t| pay * annuity(i, t * 12.0) + res * (1.0 + i).powf(-t * 12.0) - val, 0.01, 1000.0)?,
                _ => return None,
            }
        }
        2 => match want { 0 => v[1] * v[2], 1 => v[0] / v[2], _ => v[0] / v[1] },
        3 => match want { 0 => v[1] / v[2] * 100.0, 1 => v[0] * v[2] / 100.0, _ => v[1] / v[0] * 100.0 },
        _ => return None,
    };
    res.is_finite().then_some(res)
}

fn bisect(f: impl Fn(f64) -> f64, mut lo: f64, mut hi: f64) -> Option<f64> {
    let (mut flo, fhi) = (f(lo), f(hi));
    if flo.signum() == fhi.signum() { return None; }
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        let fm = f(mid);
        if fm.signum() == flo.signum() { lo = mid; flo = fm; } else { hi = mid; }
    }
    Some((lo + hi) / 2.0)
}

// ------------------------------------------------------------------ UI
#[derive(Clone, Copy, PartialEq)]
enum Panel { None, Unit, Date, Sheet(usize) }

struct Ui {
    win: gtk::ApplicationWindow,
    c: RefCell<Calc>,
    disp: gtk::Label,
    expr: gtk::Label,
    mem: gtk::Label,
    body: gtk::Box,
    hist_sw: gtk::ScrolledWindow,
    hist_list: gtk::ListBox,
    stats_sw: gtk::ScrolledWindow,
    stats_list: gtk::ListBox,
    bits: gtk::Label,
    keys: RefCell<Vec<(String, gtk::Button)>>,
    radios: RefCell<Vec<(String, gtk::RadioButton)>>,
    history_on: Cell<bool>,
    panel_box: gtk::Box,
    panel: Cell<Panel>,
    /// re-computes the panel from the current number (unit conversion "From")
    panel_refresh: RefCell<Option<Box<dyn Fn(f64)>>>,
    mode_items: RefCell<Vec<(Mode, gtk::RadioMenuItem)>>,
    hist_item: RefCell<Option<gtk::CheckMenuItem>>,
}

const CSS: &str = r#"
.calc-win { background-image: linear-gradient(to bottom, #e9eff7, #d9e4f1); }
.calc-display { background-image: linear-gradient(to bottom, #f0f6fd, #ffffff 40%); border: 1px solid #8e9fb5; border-radius: 3px; padding: 3px 6px; box-shadow: inset 0 1px 2px rgba(0,0,0,0.15); }
.calc-expr { color: #5b6170; font-size: 9pt; }
.calc-num { color: #000; font-size: 18pt; }
.calc-mem { color: #000; font-size: 9pt; }
.calc-list, .calc-list row { background-color: transparent; }
.calc-list row { padding: 0 2px; }
.calc-list row:selected { background-color: #cde6f7; color: #000; }
.calc-histsep { border-bottom: 1px solid #d6dfe9; }
button.ck { min-width: 32px; min-height: 25px; padding: 0 2px; border-radius: 3px; border: 1px solid #8a9ab1;
  background-image: linear-gradient(to bottom, #f6f8fb, #e6ebf3 49%, #dbe2ed 50%, #e5ebf3); box-shadow: inset 0 0 0 1px rgba(255,255,255,0.8); color: #000; }
button.ck:hover { background-image: linear-gradient(to bottom, #fff9e8, #fdeec6 49%, #fbdb8c 50%, #fde9a6); border-color: #c7a24e; }
button.ck:active, button.ck.on { background-image: linear-gradient(to bottom, #f8d891, #f5c35b 49%, #eda92b 50%, #f2bf54); border-color: #a3792b; }
button.ck:disabled { background-image: linear-gradient(to bottom, #f4f5f7, #eceef1); border-color: #c5ccd6; color: #9da5af; }
button.ck.eq { min-height: 54px; }
button.ck.wide { min-width: 68px; }
.calc-bits { font-family: "Consolas", monospace; font-size: 8pt; color: #2b3a4f; background-color: rgba(255,255,255,0.4); border: 1px solid #b6c3d4; border-radius: 3px; padding: 3px 6px; }
.calc-panel { border-left: 1px solid #b6c3d4; padding-left: 10px; }
.calc-panel label.head { color: #1e3287; }
.calc-panel label.result { font-weight: bold; }
"#;

impl Ui {
    fn refresh(&self) {
        let c = self.c.borrow();
        let txt = if c.error { c.entry.clone() } else { c.grouped(&c.entry) };
        self.disp.set_text(&txt);
        let mut e = c.expr_text.clone();
        if let Some(u) = &c.pending_unary { e.push_str(u); }
        if c.mode == Mode::Statistics && e.is_empty() { e = format!("Count = {}", c.stats.len()); }
        self.expr.set_text(&e);
        self.mem.set_text(if c.has_mem { "M" } else { "" });
        if c.mode == Mode::Programmer {
            // Win7's bit display: two rows of 32 bits with the bit numbers under them; bits outside
            // the word size are greyed out
            let u = (c.value() as i64 as u64) & c.mask();
            let mut s = String::new();
            for row in [1u32, 0] {
                let used = c.bits() > row * 32;
                let bits: Vec<String> = (0..8).rev().map(|nib| format!("{:04b}", (u >> (row * 32 + nib * 4)) & 0xf)).collect();
                let line = bits.join(" ");
                let (hi, mid, lo) = (row * 32 + 31, row * 32 + 15, row * 32);
                let labels = format!("{:<20}{:<17}{:>2}", hi, mid, lo);
                let body = format!("{line}\n{labels}");
                if used { s.push_str(&glib::markup_escape_text(&body)); } else { s.push_str(&format!("<span foreground='#a8b2c0'>{}</span>", glib::markup_escape_text(&body))); }
                if row == 1 { s.push('\n'); }
            }
            self.bits.set_markup(&s);
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
                "sqrt" | "pct" | "recip" | "Exp" | "fe" => !prog,
                _ => true,
            };
            b.set_sensitive(ok);
            if k == "Inv" { if c.inv { b.style_context().add_class("on") } else { b.style_context().remove_class("on") } }
            if k == "fe" { if c.fe { b.style_context().add_class("on") } else { b.style_context().remove_class("on") } }
        }
        // Inv relabels the functions it inverts
        for (k, b) in self.keys.borrow().iter() {
            let l = match (k.as_str(), c.inv) {
                ("sin", true) => "sin⁻¹", ("cos", true) => "cos⁻¹", ("tan", true) => "tan⁻¹",
                ("sinh", true) => "sinh⁻¹", ("cosh", true) => "cosh⁻¹", ("tanh", true) => "tanh⁻¹",
                ("ln", true) => "eˣ", ("int", true) => "Frac", ("dms", true) => "deg", ("pi", true) => "2*π", ("sq", true) => "√", ("cube", true) => "∛x",
                ("sin", false) => "sin", ("cos", false) => "cos", ("tan", false) => "tan", ("sinh", false) => "sinh", ("cosh", false) => "cosh", ("tanh", false) => "tanh",
                ("ln", false) => "ln", ("int", false) => "Int", ("dms", false) => "dms", ("pi", false) => "π", ("sq", false) => "x²", ("cube", false) => "x³",
                _ => continue,
            };
            b.set_label(l);
        }
        let v = c.value();
        drop(c);
        if let Some(f) = self.panel_refresh.borrow().as_ref() { f(v); }
    }

    fn press(self: &Rc<Self>, k: &str) {
        {
            let mut c = self.c.borrow_mut();
            let prog = c.mode == Mode::Programmer;
            match k {
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "." => c.digit(k.chars().next().unwrap()),
                "A" | "B" | "C" | "D" | "E" | "F" if prog && c.base == Base::Hex => c.digit(k.chars().next().unwrap()),
                "back" => c.backspace(),
                "CE" => c.clear_entry(),
                "Cl" => c.clear(),
                "neg" => c.negate(),
                "+" => c.binop(Op::Add), "-" => c.binop(Op::Sub), "*" => c.binop(Op::Mul), "/" | "Quot" => c.binop(Op::Div),
                "xy" => if c.inv { c.inv = false; c.binop(Op::Root) } else { c.binop(Op::Pow) },
                "yroot" => c.binop(Op::Root), "Mod" => c.binop(Op::Mod),
                "And" => c.binop(Op::And), "Or" => c.binop(Op::Or), "Xor" => c.binop(Op::Xor), "Lsh" => c.binop(Op::Lsh), "Rsh" => c.binop(Op::Rsh),
                "(" => c.paren(true), ")" => c.paren(false),
                "=" => c.equals(),
                "MC" | "MR" | "MS" | "M+" | "M-" => c.mem(k),
                "Inv" => c.inv = !c.inv,
                "fe" => { c.fe = !c.fe; if !c.error && c.fresh { let v = c.value(); c.entry = c.fmt(v); } }
                "Exp" => { if c.error { c.clear(); } if c.fresh { c.entry = "0".into(); c.fresh = false; } if !c.entry.contains('e') { c.entry.push_str("e+"); } }
                "Add" => c.stat_add(),
                "CAD" => { c.stats.clear(); c.clear(); }
                "avg" | "avgsq" | "sum" | "sumsq" | "sdn" | "sdn1" => c.stat(k),
                f => c.unary(f),
            }
            if c.error { drop(c); sounds::play(sounds::Event::DefaultBeep); }
        }
        self.refresh();
        self.refresh_lists();
    }

    fn refresh_lists(&self) {
        let c = self.c.borrow();
        if self.history_on.get() && c.mode != Mode::Programmer {
            for ch in self.hist_list.children() { self.hist_list.remove(&ch); }
            for (_, e, r) in c.history.iter().filter(|h| h.0 == c.mode) {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let le = gtk::Label::new(Some(e));
                le.set_xalign(1.0);
                le.set_hexpand(true);
                le.set_ellipsize(gtk::pango::EllipsizeMode::Start);
                let lr = gtk::Label::new(Some(&c.grouped(r)));
                lr.set_xalign(1.0);
                row.pack_start(&le, true, true, 0);
                row.pack_start(&lr, false, false, 0);
                let lrow = gtk::ListBoxRow::new();
                lrow.add(&row);
                unsafe { lrow.set_data("result", r.clone()); lrow.set_data("expr", e.clone()); }
                self.hist_list.add(&lrow);
            }
            self.hist_list.show_all();
            // newest at the bottom, scrolled into view
            let adj = self.hist_sw.vadjustment();
            glib::idle_add_local_once(move || adj.set_value(adj.upper()));
        }
        if c.mode == Mode::Statistics {
            for ch in self.stats_list.children() { self.stats_list.remove(&ch); }
            for v in &c.stats {
                let l = gtk::Label::new(Some(&c.grouped(&c.fmt(*v))));
                l.set_xalign(1.0);
                self.stats_list.add(&l);
            }
            self.stats_list.show_all();
            let adj = self.stats_sw.vadjustment();
            glib::idle_add_local_once(move || adj.set_value(adj.upper()));
        }
    }

    fn key(self: &Rc<Self>, label: &str, code: &str, tip: &str) -> gtk::Button {
        let b = gtk::Button::with_label(label);
        b.style_context().add_class("ck");
        b.set_focus_on_click(false);
        b.set_can_focus(false);
        if !tip.is_empty() { b.set_tooltip_text(Some(tip)); }
        let (u, c) = (self.clone(), code.to_string());
        b.connect_clicked(move |_| u.press(&c));
        self.keys.borrow_mut().push((code.to_string(), b.clone()));
        b
    }

    fn radio_group(self: &Rc<Self>, items: &[(&str, &str)], vertical: bool, active: usize, f: Rc<dyn Fn(usize)>) -> gtk::Frame {
        let b = gtk::Box::new(if vertical { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal }, if vertical { 1 } else { 10 });
        b.set_border_width(3);
        let mut first: Option<gtk::RadioButton> = None;
        for (i, (l, id)) in items.iter().enumerate() {
            let r = match &first { None => gtk::RadioButton::with_label(l), Some(g) => gtk::RadioButton::with_label_from_widget(g, l) };
            if first.is_none() { first = Some(r.clone()); }
            r.set_active(i == active);
            r.set_can_focus(false);
            let f = f.clone();
            r.connect_toggled(move |r| if r.is_active() { f(i) });
            b.pack_start(&r, false, false, 0);
            self.radios.borrow_mut().push((id.to_string(), r));
        }
        let fr = gtk::Frame::new(None);
        fr.add(&b);
        fr
    }

    fn select_radio(&self, id: &str) {
        if let Some((_, r)) = self.radios.borrow().iter().find(|(i, _)| i == id) { r.set_active(true); }
    }

    fn build_mode(self: &Rc<Self>) {
        for ch in self.body.children() { self.body.remove(&ch); }
        self.keys.borrow_mut().clear();
        self.radios.borrow_mut().clear();
        let mode = self.c.borrow().mode;
        let grid = gtk::Grid::new();
        grid.set_row_spacing(5);
        grid.set_column_spacing(5);
        let k = |l: &str, c: &str, tip: &str| self.key(l, c, tip);
        // standard block (5 cols), placed at column offset `x`
        let std_block = |g: &gtk::Grid, x: i32, y: i32| {
            let rows: [[(&str, &str, &str); 5]; 5] = [
                [("MC", "MC", "Clear memory (Ctrl+L)"), ("MR", "MR", "Recall memory (Ctrl+R)"), ("MS", "MS", "Store in memory (Ctrl+M)"), ("M+", "M+", "Add to memory (Ctrl+P)"), ("M-", "M-", "Subtract from memory (Ctrl+Q)")],
                [("←", "back", "Backspace"), ("CE", "CE", "Clear entry (Del)"), ("C", "Cl", "Clear (Esc)"), ("±", "neg", "Negate (F9)"), ("√", "sqrt", "Square root (@)")],
                [("7", "7", ""), ("8", "8", ""), ("9", "9", ""), ("/", "/", "Divide (/)"), ("%", "pct", "Percentage (%)")],
                [("4", "4", ""), ("5", "5", ""), ("6", "6", ""), ("*", "*", "Multiply (*)"), ("1/x", "recip", "Reciprocal (R)")],
                [("1", "1", ""), ("2", "2", ""), ("3", "3", ""), ("-", "-", "Subtract (-)"), ("=", "=", "Equals (Enter)")],
            ];
            for (r, row) in rows.iter().enumerate() {
                for (col, (l, c, t)) in row.iter().enumerate() {
                    let b = k(l, c, t);
                    if *c == "=" { b.style_context().add_class("eq"); g.attach(&b, x + col as i32, y + r as i32, 1, 2); continue; }
                    g.attach(&b, x + col as i32, y + r as i32, 1, 1);
                }
            }
            let zero = k("0", "0", "");
            zero.style_context().add_class("wide");
            g.attach(&zero, x, y + 5, 2, 1);
            g.attach(&k(".", ".", "Decimal separator"), x + 2, y + 5, 1, 1);
            g.attach(&k("+", "+", "Add (+)"), x + 3, y + 5, 1, 1);
        };
        match mode {
            Mode::Standard => std_block(&grid, 0, 0),
            Mode::Scientific => {
                let u = self.clone();
                let angle = self.c.borrow().angle as usize;
                let fr = self.radio_group(&[("Degrees", "deg"), ("Radians", "rad"), ("Grads", "grad")], false, angle, Rc::new(move |i| { u.c.borrow_mut().angle = i as u8; }));
                grid.attach(&fr, 0, 0, 5, 1);
                let sci: [[(&str, &str, &str); 5]; 5] = [
                    [("", "", ""), ("Inv", "Inv", "Inverse (I)"), ("ln", "ln", "Natural logarithm (N)"), ("(", "(", "Open parenthesis ("), (")", ")", "Close parenthesis )")],
                    [("Int", "int", "Integer part (;)"), ("sinh", "sinh", "Hyperbolic sine (Ctrl+S)"), ("sin", "sin", "Sine (S)"), ("x²", "sq", "Square (Q)"), ("n!", "fact", "Factorial (!)")],
                    [("dms", "dms", "Degree-minute-second (M)"), ("cosh", "cosh", "Hyperbolic cosine (Ctrl+O)"), ("cos", "cos", "Cosine (O)"), ("xʸ", "xy", "x to the power of y (Y)"), ("ʸ√x", "yroot", "y root of x (Ctrl+Y)")],
                    [("π", "pi", "Pi (P)"), ("tanh", "tanh", "Hyperbolic tangent (Ctrl+T)"), ("tan", "tan", "Tangent (T)"), ("x³", "cube", "Cube (#)"), ("∛x", "cbrt", "Cube root (Ctrl+B)")],
                    [("F-E", "fe", "Scientific notation (V)"), ("Exp", "Exp", "Exponential notation (X)"), ("Mod", "Mod", "Modulo (D)"), ("log", "log", "Logarithm (L)"), ("10ˣ", "tenx", "10 to the power of x (Ctrl+G)")],
                ];
                for (r, row) in sci.iter().enumerate() {
                    for (col, (l, c, t)) in row.iter().enumerate() {
                        if c.is_empty() { continue; }
                        grid.attach(&k(l, c, t), col as i32, r as i32 + 1, 1, 1);
                    }
                }
                std_block(&grid, 5, 0);
            }
            Mode::Programmer => {
                let u = self.clone();
                let base = match self.c.borrow().base { Base::Hex => 0, Base::Dec => 1, Base::Oct => 2, Base::Bin => 3 };
                let fr = self.radio_group(&[("Hex", "hex"), ("Dec", "dec"), ("Oct", "oct"), ("Bin", "bin")], true, base, Rc::new(move |i| {
                    u.c.borrow_mut().set_base([Base::Hex, Base::Dec, Base::Oct, Base::Bin][i]);
                    u.refresh();
                }));
                grid.attach(&fr, 0, 0, 1, 3);
                let u = self.clone();
                let word = match self.c.borrow().word { Word::Qword => 0, Word::Dword => 1, Word::Word => 2, Word::Byte => 3 };
                let fr = self.radio_group(&[("Qword", "qword"), ("Dword", "dword"), ("Word", "word"), ("Byte", "byte")], true, word, Rc::new(move |i| {
                    let mut c = u.c.borrow_mut();
                    let v = c.value();
                    c.word = [Word::Qword, Word::Dword, Word::Word, Word::Byte][i];
                    let m = c.mask();
                    let v = c.sign_extend((v as i64 as u64) & m) as f64;
                    if !c.error { c.entry = c.fmt(v); }
                    drop(c);
                    u.refresh();
                }));
                grid.attach(&fr, 0, 3, 1, 3);
                let prog: [[(&str, &str, &str); 3]; 6] = [
                    [("Quot", "Quot", "Integer division"), ("Mod", "Mod", "Modulo (%)"), ("A", "A", "")], [("(", "(", ""), (")", ")", ""), ("B", "B", "")],
                    [("RoL", "rol", "Rotate left (J)"), ("RoR", "ror", "Rotate right (K)"), ("C", "C", "")], [("Or", "Or", "Or (|)"), ("Xor", "Xor", "Exclusive or (^)"), ("D", "D", "")],
                    [("Lsh", "Lsh", "Left shift (<)"), ("Rsh", "Rsh", "Right shift (>)"), ("E", "E", "")], [("Not", "not", "Not (~)"), ("And", "And", "And (&)"), ("F", "F", "")],
                ];
                for (r, row) in prog.iter().enumerate() {
                    for (col, (l, c, t)) in row.iter().enumerate() {
                        grid.attach(&k(l, c, t), col as i32 + 1, r as i32, 1, 1);
                    }
                }
                std_block(&grid, 4, 0);
            }
            Mode::Statistics => {
                let rows: [[(&str, &str, &str); 5]; 6] = [
                    [("MC", "MC", "Clear memory (Ctrl+L)"), ("MR", "MR", "Recall memory (Ctrl+R)"), ("MS", "MS", "Store in memory (Ctrl+M)"), ("M+", "M+", "Add to memory (Ctrl+P)"), ("M-", "M-", "Subtract from memory (Ctrl+Q)")],
                    [("←", "back", "Backspace"), ("CE", "CE", "Clear entry (Del)"), ("C", "Cl", "Clear (Esc)"), ("±", "neg", "Negate (F9)"), ("Exp", "Exp", "Exponential notation (X)")],
                    [("7", "7", ""), ("8", "8", ""), ("9", "9", ""), ("x̄", "avg", "Average (A)"), ("x̄²", "avgsq", "Average of squares (Ctrl+A)")],
                    [("4", "4", ""), ("5", "5", ""), ("6", "6", ""), ("Σx", "sum", "Sum (S)"), ("Σx²", "sumsq", "Sum of squares (Ctrl+S)")],
                    [("1", "1", ""), ("2", "2", ""), ("3", "3", ""), ("σn", "sdn", "Population standard deviation (Ctrl+T)"), ("σn-1", "sdn1", "Standard deviation (T)")],
                    [("0", "0", ""), ("", "", ""), (".", ".", "Decimal separator"), ("CAD", "CAD", "Clear all data (D)"), ("Add", "Add", "Add to data set (Ins)")],
                ];
                for (r, row) in rows.iter().enumerate() {
                    for (col, (l, c, t)) in row.iter().enumerate() {
                        if c.is_empty() { continue; }
                        let b = k(l, c, t);
                        if *c == "0" { b.style_context().add_class("wide"); grid.attach(&b, col as i32, r as i32, 2, 1); continue; }
                        grid.attach(&b, col as i32, r as i32, 1, 1);
                    }
                }
            }
        }
        self.bits.set_visible(mode == Mode::Programmer);
        self.stats_sw.set_visible(mode == Mode::Statistics);
        self.hist_sw.set_visible(self.history_on.get() && mode != Mode::Programmer);
        if let Some(h) = self.hist_item.borrow().as_ref() { h.set_sensitive(mode != Mode::Programmer); }
        self.body.pack_start(&grid, false, false, 0);
        self.body.show_all();
        self.refresh();
        self.refresh_lists();
        self.shrink();
    }

    fn set_mode(self: &Rc<Self>, m: Mode) {
        {
            let mut c = self.c.borrow_mut();
            if c.mode == m { return; }
            // switching modes starts from 0, like Win7
            c.mode = m;
            c.clear();
            c.inv = false;
        }
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("calc-mode"), m.key());
        if m == Mode::Programmer && matches!(self.panel.get(), Panel::Unit) { self.show_panel(Panel::None); }
        self.build_mode();
    }

    /// Fit the window to its contents again (after the layout settled).
    fn shrink(&self) {
        self.win.resize(1, 1);
        let w = self.win.clone();
        glib::idle_add_local_once(move || w.resize(1, 1));
    }

    // ---------------------------------------------------------------- side panels
    fn show_panel(self: &Rc<Self>, p: Panel) {
        for ch in self.panel_box.children() { self.panel_box.remove(&ch); }
        *self.panel_refresh.borrow_mut() = None;
        self.panel.set(p);
        let w: Option<gtk::Widget> = match p {
            Panel::None => None,
            Panel::Unit => Some(self.unit_panel().upcast()),
            Panel::Date => Some(self.date_panel().upcast()),
            Panel::Sheet(i) => Some(self.sheet_panel(i).upcast()),
        };
        if let Some(w) = w { self.panel_box.pack_start(&w, true, true, 0); self.panel_box.show_all(); self.panel_box.set_visible(true); } else { self.panel_box.set_visible(false); }
        self.refresh();
        self.shrink();
    }

    fn head(text: &str) -> gtk::Label {
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        l.style_context().add_class("head");
        l
    }

    fn unit_panel(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
        b.set_size_request(240, -1);
        b.pack_start(&Self::head("Select the type of unit you want to convert"), false, false, 0);
        let cat = gtk::ComboBoxText::new();
        for (n, _) in UNITS { cat.append_text(n); }
        b.pack_start(&cat, false, false, 0);
        b.pack_start(&Self::head("From"), false, false, 6);
        let from_v = gtk::Entry::new();
        from_v.set_editable(false);
        from_v.set_can_focus(false);
        from_v.set_alignment(1.0);
        let from_u = gtk::ComboBoxText::new();
        b.pack_start(&from_v, false, false, 0);
        b.pack_start(&from_u, false, false, 0);
        b.pack_start(&Self::head("To"), false, false, 6);
        let to_v = gtk::Entry::new();
        to_v.set_editable(false);
        to_v.set_alignment(1.0);
        let to_u = gtk::ComboBoxText::new();
        b.pack_start(&to_v, false, false, 0);
        b.pack_start(&to_u, false, false, 0);
        let saved: Vec<usize> = std::fs::read_to_string(config::conf_dir().join("calc-units")).unwrap_or_default().split_whitespace().filter_map(|x| x.parse().ok()).collect();
        let fill = {
            let (from_u, to_u) = (from_u.clone(), to_u.clone());
            move |i: usize, f: usize, t: usize| {
                from_u.remove_all();
                to_u.remove_all();
                for (n, _) in UNITS[i].1 { from_u.append_text(n); to_u.append_text(n); }
                let n = UNITS[i].1.len();
                from_u.set_active(Some(f.min(n - 1) as u32));
                to_u.set_active(Some(t.min(n - 1) as u32));
            }
        };
        let compute = {
            let (cat, from_u, to_u, from_v, to_v, u) = (cat.clone(), from_u.clone(), to_u.clone(), from_v.clone(), to_v.clone(), Rc::downgrade(self));
            Rc::new(move |v: f64| {
                let (Some(c), Some(f), Some(t)) = (cat.active(), from_u.active(), to_u.active()) else { return };
                let Some(u) = u.upgrade() else { return };
                let calc = u.c.borrow();
                from_v.set_text(&calc.grouped(&fmt_num(v, false)));
                to_v.set_text(&calc.grouped(&fmt_num(convert(c as usize, f as usize, t as usize, v), false)));
                let _ = std::fs::write(config::conf_dir().join("calc-units"), format!("{c} {f} {t}"));
            })
        };
        cat.set_active(Some(*saved.first().unwrap_or(&3) as u32));
        fill(saved.first().copied().unwrap_or(3).min(UNITS.len() - 1), saved.get(1).copied().unwrap_or(9), saved.get(2).copied().unwrap_or(4));
        {
            let (fill, u) = (fill.clone(), Rc::downgrade(self));
            cat.connect_changed(move |c| { if let Some(i) = c.active() { fill(i as usize, 0, 1); } if let Some(u) = u.upgrade() { u.refresh(); } });
        }
        for cb in [&from_u, &to_u] { let u = Rc::downgrade(self); cb.connect_changed(move |_| { if let Some(u) = u.upgrade() { u.refresh(); } }); }
        let c2 = compute.clone();
        *self.panel_refresh.borrow_mut() = Some(Box::new(move |v| c2(v)));
        b
    }

    fn date_panel(self: &Rc<Self>) -> gtk::Box {
        use chrono::Datelike;
        let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
        b.set_size_request(260, -1);
        b.pack_start(&Self::head("Select the date calculation you want"), false, false, 0);
        let kind = gtk::ComboBoxText::new();
        kind.append_text("Calculate the difference between two dates");
        kind.append_text("Add or subtract days to a specified date");
        kind.set_active(Some(0));
        b.pack_start(&kind, false, false, 0);
        // a date field: button showing the date, calendar drop-down
        let date_field = |d: chrono::NaiveDate| -> (gtk::MenuButton, gtk::Calendar) {
            let cal = gtk::Calendar::new();
            cal.select_month(d.month0(), d.year() as u32);
            cal.select_day(d.day());
            let mb = gtk::MenuButton::new();
            mb.set_label(&d.format("%-m/%-d/%Y").to_string());
            let pop = gtk::Popover::new(Some(&mb));
            pop.add(&cal);
            cal.show();
            mb.set_popover(Some(&pop));
            { let mb2 = mb.clone(); cal.connect_day_selected(move |c| { let (y, m, d) = c.date(); mb2.set_label(&format!("{}/{}/{}", m + 1, d, y)); }); }
            { let p2 = pop.clone(); cal.connect_day_selected_double_click(move |_| p2.popdown()); }
            (mb, cal)
        };
        let get = |c: &gtk::Calendar| { let (y, m, d) = c.date(); chrono::NaiveDate::from_ymd_opt(y as i32, m + 1, d) };
        let today = chrono::Local::now().date_naive();
        let stack = gtk::Stack::new();
        // difference
        let diff = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let (fb, fc) = date_field(today);
        let (tb, tc) = date_field(today);
        diff.pack_start(&Self::head("From"), false, false, 4);
        diff.pack_start(&fb, false, false, 0);
        diff.pack_start(&Self::head("To"), false, false, 4);
        diff.pack_start(&tb, false, false, 0);
        let calc1 = gtk::Button::with_mnemonic("_Calculate");
        calc1.set_halign(gtk::Align::End);
        diff.pack_start(&calc1, false, false, 6);
        diff.pack_start(&Self::head("Difference\n(years, months, weeks, days)"), false, false, 0);
        let r1 = gtk::Label::new(Some(""));
        r1.set_xalign(0.0);
        r1.set_line_wrap(true);
        r1.style_context().add_class("result");
        diff.pack_start(&r1, false, false, 0);
        diff.pack_start(&Self::head("(days)"), false, false, 2);
        let r2 = gtk::Label::new(Some(""));
        r2.set_xalign(0.0);
        r2.style_context().add_class("result");
        diff.pack_start(&r2, false, false, 0);
        {
            let (fc, tc, r1, r2) = (fc.clone(), tc.clone(), r1.clone(), r2.clone());
            calc1.connect_clicked(move |_| {
                let (Some(a), Some(b)) = (get(&fc), get(&tc)) else { return };
                let (text, days) = date_diff(a, b);
                r1.set_text(&text);
                r2.set_text(&if days == 0 { "Same dates".to_string() } else { format!("{} day{}", group_int(days), if days == 1 { "" } else { "s" }) });
            });
        }
        stack.add_named(&diff, "diff");
        // add / subtract
        let addb = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let (sb, sc) = date_field(today);
        addb.pack_start(&Self::head("From"), false, false, 4);
        addb.pack_start(&sb, false, false, 0);
        let rb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let add = gtk::RadioButton::with_mnemonic("_Add");
        let sub = gtk::RadioButton::with_mnemonic_from_widget(&add, "_Subtract");
        rb.pack_start(&add, false, false, 0);
        rb.pack_start(&sub, false, false, 0);
        addb.pack_start(&rb, false, false, 4);
        let g = gtk::Grid::new();
        g.set_column_spacing(6);
        g.set_row_spacing(2);
        let spins: Vec<gtk::SpinButton> = ["Year(s)", "Month(s)", "Day(s)"].iter().enumerate().map(|(i, l)| {
            let s = gtk::SpinButton::with_range(0.0, if i == 2 { 999_999.0 } else { 999.0 }, 1.0);
            let lab = gtk::Label::new(Some(l));
            lab.set_xalign(0.0);
            g.attach(&lab, i as i32, 0, 1, 1);
            g.attach(&s, i as i32, 1, 1, 1);
            s
        }).collect();
        addb.pack_start(&g, false, false, 0);
        let calc2 = gtk::Button::with_mnemonic("_Calculate");
        calc2.set_halign(gtk::Align::End);
        addb.pack_start(&calc2, false, false, 6);
        addb.pack_start(&Self::head("Date"), false, false, 0);
        let r3 = gtk::Label::new(Some(""));
        r3.set_xalign(0.0);
        r3.style_context().add_class("result");
        addb.pack_start(&r3, false, false, 0);
        {
            let (sc, sub, spins, r3) = (sc.clone(), sub.clone(), spins.clone(), r3.clone());
            calc2.connect_clicked(move |_| {
                let Some(a) = get(&sc) else { return };
                match add_date(a, spins[0].value() as i32, spins[1].value() as i32, spins[2].value() as i64, sub.is_active()) {
                    Some(d) => r3.set_text(&d.format("%A, %B %-d, %Y").to_string()),
                    None => r3.set_text("Invalid input"),
                }
            });
        }
        stack.add_named(&addb, "add");
        { let st = stack.clone(); kind.connect_changed(move |k| st.set_visible_child_name(if k.active() == Some(1) { "add" } else { "diff" })); }
        b.pack_start(&stack, false, false, 0);
        b
    }

    fn sheet_panel(self: &Rc<Self>, i: usize) -> gtk::Box {
        let (_, fields) = SHEETS[i];
        let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
        b.set_size_request(240, -1);
        b.pack_start(&Self::head("Select the value you want to calculate"), false, false, 0);
        let want = gtk::ComboBoxText::new();
        for f in fields.iter() { want.append_text(f); }
        want.set_active(Some(fields.len() as u32 - 1));
        b.pack_start(&want, false, false, 0);
        let mut entries = vec![];
        let mut labels = vec![];
        for f in fields.iter() {
            let l = Self::head(f);
            let e = gtk::Entry::new();
            e.set_alignment(1.0);
            e.set_input_purpose(gtk::InputPurpose::Number);
            b.pack_start(&l, false, false, 2);
            b.pack_start(&e, false, false, 0);
            labels.push(l);
            entries.push(e);
        }
        let calc = gtk::Button::with_mnemonic("_Calculate");
        calc.set_halign(gtk::Align::End);
        b.pack_start(&calc, false, false, 6);
        // the value being calculated is read-only
        let sync = {
            let entries = entries.clone();
            move |w: &gtk::ComboBoxText| { let a = w.active().unwrap_or(0) as usize; for (j, e) in entries.iter().enumerate() { e.set_editable(j != a); e.set_can_focus(j != a); } }
        };
        sync(&want);
        want.connect_changed(sync);
        {
            let (entries, want) = (entries.clone(), want.clone());
            calc.connect_clicked(move |_| {
                let a = want.active().unwrap_or(0) as usize;
                let vals: Vec<f64> = entries.iter().map(|e| e.text().replace(',', "").trim().parse().unwrap_or(0.0)).collect();
                match solve_sheet(i, a, &vals) {
                    Some(v) => entries[a].set_text(&if fields[a].contains("rate") || fields[a].contains("economy") || fields[a].contains("Term") { format!("{v:.2}") } else { format!("{:.2}", (v * 100.0).round() / 100.0) }),
                    None => { entries[a].set_text("Invalid input"); sounds::play(sounds::Event::DefaultBeep); }
                }
            });
        }
        b
    }

    fn copy(&self) {
        let c = self.c.borrow();
        let t = if c.error { c.entry.clone() } else { c.entry.replace(',', "") };
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&t);
    }

    fn paste(self: &Rc<Self>) {
        let u = self.clone();
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).request_text(move |_, t| {
            let Some(t) = t else { return };
            let t = t.trim().to_string();
            // a plain number goes straight into the display
            let prog = u.c.borrow().mode == Mode::Programmer;
            let plain = t.replace([',', ' '], "");
            let is_num = if prog { let r = u.c.borrow().base.radix(); !plain.is_empty() && plain.trim_start_matches('-').chars().all(|c| c.is_digit(r)) } else { plain.parse::<f64>().is_ok() };
            if is_num {
                let mut c = u.c.borrow_mut();
                if c.error { c.clear(); }
                c.fresh = true;
                for ch in plain.chars() { if ch == '-' { continue; } c.digit(ch.to_ascii_uppercase()); }
                if plain.starts_with('-') { c.negate(); }
                drop(c);
                u.refresh();
                return;
            }
            // otherwise an expression, key by key
            for ch in t.chars() {
                let k = match ch {
                    '0'..='9' | '.' | '+' | '-' | '*' | '/' | '(' | ')' => ch.to_string(),
                    'a'..='f' | 'A'..='F' if prog => ch.to_ascii_uppercase().to_string(),
                    '=' | '\n' => "=".into(),
                    '%' => "pct".into(),
                    _ => continue,
                };
                u.press(&k);
            }
        });
    }

    /// Win7 Calculator keyboard map (per mode). Returns true when handled.
    fn keypress(self: &Rc<Self>, e: &gdk::EventKey) -> bool {
        use gdk::keys::constants as K;
        let kv = e.keyval();
        let st = e.state();
        let ctrl = st.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = st.contains(gdk::ModifierType::MOD1_MASK);
        if alt { return false; }
        // typing into a panel field (worksheet entries, spin buttons) goes to that field
        if let Some(f) = self.win.focused_widget() { if f.is::<gtk::Entry>() && f.downcast_ref::<gtk::Entry>().map(|e| e.is_editable()).unwrap_or(false) { return false; } }
        let mode = self.c.borrow().mode;
        let ch = kv.to_unicode().map(|c| c.to_ascii_lowercase());
        let code: Option<&str> = if ctrl {
            match ch {
                Some('m') => Some("MS"), Some('p') => Some("M+"), Some('q') => Some("M-"), Some('r') => Some("MR"), Some('l') => Some("MC"),
                Some('s') if mode == Mode::Scientific => Some("sinh"), Some('o') if mode == Mode::Scientific => Some("cosh"), Some('t') if mode == Mode::Scientific => Some("tanh"),
                Some('y') if mode == Mode::Scientific => Some("yroot"), Some('b') if mode == Mode::Scientific => Some("cbrt"), Some('g') if mode == Mode::Scientific => Some("tenx"),
                Some('a') if mode == Mode::Statistics => Some("avgsq"), Some('s') if mode == Mode::Statistics => Some("sumsq"), Some('t') if mode == Mode::Statistics => Some("sdn"),
                _ => None,
            }
        } else {
            match kv {
                k if k == K::Return || k == K::KP_Enter => Some(if mode == Mode::Statistics { "Add" } else { "=" }),
                k if (k == K::Insert || k == K::KP_Insert) && mode == Mode::Statistics => Some("Add"),
                k if k == K::equal => Some("="),
                k if k == K::BackSpace => Some("back"),
                k if k == K::Escape => Some("Cl"),
                k if k == K::Delete || k == K::KP_Delete => Some("CE"),
                k if k == K::F9 => Some("neg"),
                k if k == K::KP_Add => Some("+"), k if k == K::KP_Subtract => Some("-"), k if k == K::KP_Multiply => Some("*"), k if k == K::KP_Divide => Some("/"),
                k if k == K::KP_Decimal || k == K::KP_Separator => Some("."),
                k if (k == K::F3 || k == K::F4 || k == K::F5) && mode == Mode::Scientific => { self.select_radio(if k == K::F3 { "deg" } else if k == K::F4 { "rad" } else { "grad" }); return true; }
                k if mode == Mode::Programmer && [K::F5, K::F6, K::F7, K::F8, K::F12, K::F2, K::F3, K::F4].contains(&k) => {
                    let id = if k == K::F5 { "hex" } else if k == K::F6 { "dec" } else if k == K::F7 { "oct" } else if k == K::F8 { "bin" } else if k == K::F12 { "qword" } else if k == K::F2 { "dword" } else if k == K::F3 { "word" } else { "byte" };
                    self.select_radio(id);
                    return true;
                }
                _ => match (ch, mode) {
                    (Some(c @ '0'..='9'), _) => return { self.press(&c.to_string()); true },
                    (Some('.') | Some(','), _) => Some("."),
                    (Some('+'), _) => Some("+"), (Some('-'), _) => Some("-"), (Some('*'), _) => Some("*"), (Some('/'), _) => Some("/"),
                    (Some('@'), m) if m != Mode::Programmer => Some("sqrt"),
                    (Some('%'), Mode::Programmer) => Some("Mod"),
                    (Some('%'), _) => Some("pct"),
                    (Some('r'), Mode::Standard | Mode::Scientific) => Some("recip"),
                    (Some('('), Mode::Scientific | Mode::Programmer) => Some("("), (Some(')'), Mode::Scientific | Mode::Programmer) => Some(")"),
                    (Some('i'), Mode::Scientific) => Some("Inv"), (Some('d'), Mode::Scientific) => Some("Mod"), (Some('n'), Mode::Scientific) => Some("ln"),
                    (Some(';'), Mode::Scientific) => Some("int"), (Some('s'), Mode::Scientific) => Some("sin"), (Some('o'), Mode::Scientific) => Some("cos"),
                    (Some('t'), Mode::Scientific) => Some("tan"), (Some('m'), Mode::Scientific) => Some("dms"), (Some('p'), Mode::Scientific) => Some("pi"),
                    (Some('v'), Mode::Scientific) => Some("fe"), (Some('x'), Mode::Scientific | Mode::Statistics) => Some("Exp"), (Some('q'), Mode::Scientific) => Some("sq"),
                    (Some('y'), Mode::Scientific) => Some("xy"), (Some('#'), Mode::Scientific) => Some("cube"), (Some('l'), Mode::Scientific) => Some("log"),
                    (Some('!'), Mode::Scientific) => Some("fact"),
                    (Some(c @ 'a'..='f'), Mode::Programmer) => return { self.press(&c.to_ascii_uppercase().to_string()); true },
                    (Some('k'), Mode::Programmer) => Some("ror"), (Some('j'), Mode::Programmer) => Some("rol"), (Some('<'), Mode::Programmer) => Some("Lsh"),
                    (Some('>'), Mode::Programmer) => Some("Rsh"), (Some('|'), Mode::Programmer) => Some("Or"), (Some('^'), Mode::Programmer) => Some("Xor"),
                    (Some('~'), Mode::Programmer) => Some("not"), (Some('&'), Mode::Programmer) => Some("And"),
                    (Some('a'), Mode::Statistics) => Some("avg"), (Some('s'), Mode::Statistics) => Some("sum"), (Some('t'), Mode::Statistics) => Some("sdn1"),
                    (Some('d'), Mode::Statistics) => Some("CAD"),
                    _ => None,
                },
            }
        };
        let Some(code) = code else { return false };
        // flash the button like a click
        if let Some((_, b)) = self.keys.borrow().iter().find(|(c, _)| c == code) {
            if !b.is_sensitive() { return true; }
            b.set_state_flags(gtk::StateFlags::ACTIVE, false);
            let b = b.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || b.unset_state_flags(gtk::StateFlags::ACTIVE));
        }
        self.press(code);
        true
    }
}

fn group_int(n: i64) -> String {
    let s = n.abs().to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    if n < 0 { format!("-{o}") } else { o }
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
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    outer.set_border_width(10);
    let main = gtk::Box::new(gtk::Orientation::Vertical, 8);
    // display: [history] [statistics data] expression, M + number
    let dbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dbox.style_context().add_class("calc-display");
    let list = |h: i32| {
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sw.set_size_request(-1, h);
        sw.style_context().add_class("calc-histsep");
        let l = gtk::ListBox::new();
        l.style_context().add_class("calc-list");
        l.set_selection_mode(gtk::SelectionMode::Single);
        l.set_can_focus(false);
        sw.add(&l);
        (sw, l)
    };
    let (hist_sw, hist_list) = list(96);
    let (stats_sw, stats_list) = list(96);
    let expr = gtk::Label::new(None);
    expr.set_xalign(1.0);
    expr.style_context().add_class("calc-expr");
    expr.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    expr.set_size_request(-1, 16);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let mem = gtk::Label::new(None);
    mem.style_context().add_class("calc-mem");
    mem.set_valign(gtk::Align::End);
    let disp = gtk::Label::new(Some("0"));
    disp.set_xalign(1.0);
    disp.style_context().add_class("calc-num");
    disp.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    disp.set_selectable(false);
    row.pack_start(&mem, false, false, 0);
    row.pack_start(&disp, true, true, 0);
    dbox.pack_start(&hist_sw, false, false, 0);
    dbox.pack_start(&stats_sw, false, false, 0);
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
    let panel_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel_box.style_context().add_class("calc-panel");
    outer.pack_start(&panel_box, false, false, 0);
    root.pack_start(&outer, true, true, 0);
    win.add(&root);
    let history_on = std::fs::read_to_string(config::conf_dir().join("calc-history")).map(|s| s.trim() == "1").unwrap_or(false);
    let ui = Rc::new(Ui {
        win: win.clone(), c: RefCell::new(Calc::new()), disp, expr, mem, body, hist_sw: hist_sw.clone(), hist_list: hist_list.clone(), stats_sw, stats_list, bits,
        keys: RefCell::new(vec![]), radios: RefCell::new(vec![]), history_on: Cell::new(history_on), panel_box, panel: Cell::new(Panel::None),
        panel_refresh: RefCell::new(None), mode_items: RefCell::new(vec![]), hist_item: RefCell::new(None),
    });
    ui.c.borrow_mut().grouping = config::conf_dir().join("calc-grouping").exists();
    // picking a history entry brings its result back
    {
        let u = ui.clone();
        hist_list.connect_row_activated(move |_, r| {
            let res = unsafe { r.data::<String>("result").map(|p| p.as_ref().clone()) };
            let ex = unsafe { r.data::<String>("expr").map(|p| p.as_ref().clone()) };
            if let Some(res) = res {
                let mut c = u.c.borrow_mut();
                c.clear();
                c.entry = res;
                c.fresh = true;
                c.pending_unary = ex;
                drop(c);
                u.refresh();
            }
        });
        hist_list.set_activate_on_single_click(true);
    }

    let item = |m: &gtk::Menu, l: &str, acc: &str, f: Box<dyn Fn()>| {
        let it = gtk::MenuItem::with_mnemonic(l);
        if !acc.is_empty() { let (k, md) = gtk::accelerator_parse(acc); it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE); }
        it.connect_activate(move |_| f());
        m.append(&it);
        it
    };
    // View
    let m = gtk::Menu::new();
    let mut group: Option<gtk::RadioMenuItem> = None;
    let saved = std::fs::read_to_string(config::conf_dir().join("calc-mode")).unwrap_or_default();
    let start_mode = match saved.trim() { "sci" => Mode::Scientific, "prog" => Mode::Programmer, "stat" => Mode::Statistics, _ => Mode::Standard };
    for (l, acc, mode) in [("_Standard", "<Alt>1", Mode::Standard), ("S_cientific", "<Alt>2", Mode::Scientific), ("_Programmer", "<Alt>3", Mode::Programmer), ("St_atistics", "<Alt>4", Mode::Statistics)] {
        let it = match &group { None => gtk::RadioMenuItem::with_mnemonic(l), Some(g) => gtk::RadioMenuItem::with_mnemonic_from_widget(g, Some(l)) };
        if group.is_none() { group = Some(it.clone()); }
        let (k, md) = gtk::accelerator_parse(acc);
        it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE);
        it.set_active(mode == start_mode);
        let u = ui.clone();
        it.connect_toggled(move |i| if i.is_active() { u.set_mode(mode) });
        m.append(&it);
        ui.mode_items.borrow_mut().push((mode, it));
    }
    m.append(&gtk::SeparatorMenuItem::new());
    let hist = gtk::CheckMenuItem::with_mnemonic("_History");
    let (k, md) = gtk::accelerator_parse("<Control>h");
    hist.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE);
    hist.set_active(history_on);
    {
        let u = ui.clone();
        hist.connect_toggled(move |h| {
            u.history_on.set(h.is_active());
            let _ = std::fs::create_dir_all(config::conf_dir());
            let _ = std::fs::write(config::conf_dir().join("calc-history"), if h.is_active() { "1" } else { "0" });
            u.hist_sw.set_visible(h.is_active() && u.c.borrow().mode != Mode::Programmer);
            u.refresh_lists();
            u.shrink();
        });
    }
    m.append(&hist);
    *ui.hist_item.borrow_mut() = Some(hist.clone());
    let grp = gtk::CheckMenuItem::with_mnemonic("Dig_it grouping");
    grp.set_active(ui.c.borrow().grouping);
    {
        let u = ui.clone();
        grp.connect_toggled(move |g| {
            u.c.borrow_mut().grouping = g.is_active();
            let p = config::conf_dir().join("calc-grouping");
            if g.is_active() { let _ = std::fs::create_dir_all(config::conf_dir()); let _ = std::fs::write(p, ""); } else { let _ = std::fs::remove_file(p); }
            u.refresh();
            u.refresh_lists();
        });
    }
    m.append(&grp);
    m.append(&gtk::SeparatorMenuItem::new());
    let panel_items: Rc<RefCell<Vec<(Panel, gtk::CheckMenuItem)>>> = Default::default();
    // (set_active on a check item activates it again: guard against re-entry)
    let syncing = Rc::new(Cell::new(false));
    let sync_marks = {
        let (u, items, syncing) = (ui.clone(), panel_items.clone(), syncing.clone());
        Rc::new(move || {
            syncing.set(true);
            for (q, i) in items.borrow().iter() { i.set_active(*q == u.panel.get()); }
            syncing.set(false);
        })
    };
    let add_panel = |m: &gtk::Menu, l: &str, acc: &str, p: Panel| {
        let it = gtk::CheckMenuItem::with_mnemonic(l);
        it.set_draw_as_radio(true);
        if !acc.is_empty() { let (k, md) = gtk::accelerator_parse(acc); it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE); }
        let (u, syncing, sync_marks) = (ui.clone(), syncing.clone(), sync_marks.clone());
        it.connect_activate(move |_| {
            if syncing.get() { return; }
            // choosing the panel that is shown goes back to Basic
            let target = if u.panel.get() == p { Panel::None } else { p };
            u.show_panel(target);
            sync_marks();
        });
        m.append(&it);
        panel_items.borrow_mut().push((p, it));
    };
    add_panel(&m, "_Basic", "<Control>F4", Panel::None);
    add_panel(&m, "_Unit conversion", "<Control>u", Panel::Unit);
    add_panel(&m, "_Date calculation", "<Control>e", Panel::Date);
    let ws = gtk::Menu::new();
    for (i, (n, _)) in SHEETS.iter().enumerate() { add_panel(&ws, n, "", Panel::Sheet(i)); }
    let wsi = gtk::MenuItem::with_mnemonic("_Worksheets");
    wsi.set_submenu(Some(&ws));
    m.append(&wsi);
    sync_marks();
    let top = gtk::MenuItem::with_mnemonic("_View"); top.set_submenu(Some(&m)); menubar.append(&top);
    // Edit
    let m = gtk::Menu::new();
    { let u = ui.clone(); item(&m, "_Copy", "<Control>c", Box::new(move || u.copy())); }
    { let u = ui.clone(); item(&m, "_Paste", "<Control>v", Box::new(move || u.paste())); }
    m.append(&gtk::SeparatorMenuItem::new());
    let hm = gtk::Menu::new();
    {
        let u = ui.clone();
        item(&hm, "Copy h_istory", "", Box::new(move || {
            let c = u.c.borrow();
            let t: Vec<String> = c.history.iter().filter(|h| h.0 == c.mode).map(|(_, e, r)| format!("{e} = {r}")).collect();
            gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&t.join("\n"));
        }));
    }
    { let u = ui.clone(); item(&hm, "Cl_ear", "<Control><Shift>d", Box::new(move || { { let mut c = u.c.borrow_mut(); let m = c.mode; c.history.retain(|h| h.0 != m); } for ch in u.hist_list.children() { u.hist_list.remove(&ch); } })); }
    let hi = gtk::MenuItem::with_mnemonic("_History"); hi.set_submenu(Some(&hm)); m.append(&hi);
    let top = gtk::MenuItem::with_mnemonic("_Edit"); top.set_submenu(Some(&m)); menubar.append(&top);
    // Help
    let m = gtk::Menu::new();
    item(&m, "View _Help", "F1", Box::new(|| { util::open_uri("https://en.wikipedia.org/wiki/Windows_Calculator"); }));
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); item(&m, "_About Calculator", "", Box::new(move || {
        let d = gtk::AboutDialog::new();
        d.set_transient_for(Some(&w));
        d.set_program_name("Calculator");
        d.set_comments(Some("Aerode Calculator – a Windows 7 style calculator for Linux"));
        d.set_logo_icon_name(Some(util::first_icon(&["accessories-calculator"])));
        d.run();
        d.close();
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);

    // keyboard, like Win7 (before the accelerators, which only take what we leave)
    {
        let u = ui.clone();
        win.connect_key_press_event(move |_, e| if u.keypress(e) { glib::Propagation::Stop } else { glib::Propagation::Proceed });
    }
    root.show_all();
    hist_sw.hide();
    ui.panel_box.hide();
    ui.c.borrow_mut().mode = start_mode;
    ui.build_mode();
    win.show();
}

fn main() -> glib::ExitCode {
    util::log_panics("calc");
    let app = gtk::Application::new(Some("org.aerode.Calculator"), gio::ApplicationFlags::NON_UNIQUE);
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
                "=" => c.equals(), "(" => c.paren(true), ")" => c.paren(false), "Add" => c.stat_add(),
                "avg" | "sum" | "sdn" | "sdn1" => c.stat(k),
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
    #[test] fn stats() {
        assert_eq!(run(&["2", "Add", "4", "Add", "4", "Add", "4", "Add", "5", "Add", "5", "Add", "7", "Add", "9", "Add", "avg"], Mode::Statistics), "5");
        assert_eq!(run(&["2", "Add", "4", "Add", "4", "Add", "4", "Add", "5", "Add", "5", "Add", "7", "Add", "9", "Add", "sdn"], Mode::Statistics), "2");
    }
    #[test] fn programmer_no_overflow_panic() {
        let mut c = Calc::new();
        c.mode = Mode::Programmer;
        c.entry = "-9223372036854775808".into();
        c.binop(Op::Div);
        c.entry = "-1".into();
        c.equals();
        assert!(!c.entry.is_empty());
        // digits stop at the word size
        let mut c = Calc::new();
        c.mode = Mode::Programmer;
        c.word = Word::Byte;
        for d in "999".chars() { c.digit(d); }
        assert_eq!(c.entry, "99");
    }
    #[test] fn units() {
        assert!((convert(3, 9, 4, 1.0) - 3.280839895).abs() < 1e-6);
        assert!((convert(6, 0, 1, 100.0) - 212.0).abs() < 1e-9);
    }
    #[test] fn dates() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(date_diff(d(2020, 1, 15), d(2021, 3, 1)), ("1 year, 1 month, 2 weeks".to_string(), 411));
        assert_eq!(add_date(d(2020, 1, 31), 0, 1, 0, false), Some(d(2020, 2, 29)));
    }
    #[test] fn mortgage() {
        let p = solve_sheet(0, 4, &[200000.0, 40000.0, 30.0, 6.0, 0.0]).unwrap();
        assert!((p - 959.28).abs() < 0.01);
    }
}
