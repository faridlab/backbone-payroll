//! The payslip PDF (#553): a tiny hand-rolled PDF writer plus the slip
//! layout, in the offer-letter-renderer spirit — a payslip needs text,
//! numbers and a clean page; pulling a full typesetting engine in for one
//! document type is not worth a dependency.
//!
//! The writer emits a valid single-page A4 PDF (1.4): catalog, pages,
//! page, contents (text operators), font (Helvetica + Helvetica-Bold via
//! the base-14 set — no embedding needed), and a correct xref table.
//! WinAnsi covers the slip's vocabulary (IDR amounts, names, dates);
//! characters outside it are replaced with '?' rather than breaking the
//! file.
//!
//! The layout: a header band (company, period, run status), the employee
//! block, an earnings table, a deductions table, and a bold take-home
//! line — the same facts the payslip JSON carries, in the shape a person
//! can print (#619's "download and print").

use chrono::{Datelike, Utc};
use rust_decimal::Decimal;

/// One row of a table section.
pub struct SlipRow {
    pub label: String,
    pub amount: Decimal,
    pub statutory: bool,
}

/// The facts a payslip PDF states.
pub struct PayslipPdfInput {
    pub company_name: String,
    pub period: String,
    pub employee_number: String,
    pub employee_name: String,
    pub position_title: Option<String>,
    pub working_days: Decimal,
    pub unpaid_days: Decimal,
    pub gross_pay: Decimal,
    pub total_deductions: Decimal,
    pub net_pay: Decimal,
    pub earnings: Vec<SlipRow>,
    pub deductions: Vec<SlipRow>,
}

/// Render the input to the bytes of a valid PDF document.
pub fn render_payslip_pdf(input: &PayslipPdfInput) -> Vec<u8> {
    let mut ops = String::with_capacity(4096);

    // Header band.
    let mut y = 780.0;
    bold(&mut ops, 16.0, 60.0, y, &input.company_name);
    y -= 20.0;
    text(&mut ops, 11.0, 60.0, y, &format!("Payslip — period {}", input.period));
    y -= 26.0;

    // Employee block.
    text(&mut ops, 11.0, 60.0, y, &format!("Employee: {} ({})", input.employee_name, input.employee_number));
    y -= 15.0;
    let position = input.position_title.clone().unwrap_or_else(|| "-".into());
    text(&mut ops, 11.0, 60.0, y, &format!("Position: {}", position));
    y -= 15.0;
    text(&mut ops, 11.0, 60.0, y, &format!("Working days: {}   Unpaid days: {}", input.working_days, input.unpaid_days));
    y -= 26.0;

    // Earnings.
    bold(&mut ops, 12.0, 60.0, y, "Earnings");
    y -= 16.0;
    for row in &input.earnings {
        text(&mut ops, 10.0, 70.0, y, &row.label);
        right(&mut ops, 10.0, 535.0, y, &money(row.amount));
        y -= 14.0;
    }
    line(&mut ops, 70.0, y + 4.0, 535.0);
    text(&mut ops, 10.0, 70.0, y - 8.0, "Gross pay");
    right(&mut ops, 10.0, 535.0, y - 8.0, &money(input.gross_pay));
    y -= 30.0;

    // Deductions.
    bold(&mut ops, 12.0, 60.0, y, "Deductions");
    y -= 16.0;
    for row in &input.deductions {
        text(&mut ops, 10.0, 70.0, y, &row.label);
        right(&mut ops, 10.0, 535.0, y, &money(row.amount));
        y -= 14.0;
    }
    line(&mut ops, 70.0, y + 4.0, 535.0);
    text(&mut ops, 10.0, 70.0, y - 8.0, "Total deductions");
    right(&mut ops, 10.0, 535.0, y - 8.0, &money(input.total_deductions));
    y -= 30.0;

    // Take-home.
    line(&mut ops, 60.0, y + 6.0, 535.0);
    bold(&mut ops, 13.0, 60.0, y - 12.0, "Take-home pay");
    bold_right(&mut ops, 13.0, 535.0, y - 12.0, &money(input.net_pay));

    // Footer: generation stamp.
    text(
        &mut ops,
        8.0,
        40.0,
        30.0,
        &format!("Generated {} UTC", Utc::now().format("%Y-%m-%d %H:%M")),
    );

    assemble_pdf(&ops)
}

fn money(v: Decimal) -> String {
    let s = v.round_dp(2).to_string();
    // Thousands separators for readability; WinAnsi-safe.
    let (sign, digits) = if let Some(rest) = s.strip_prefix('-') {
        ("-", rest.to_string())
    } else {
        ("", s)
    };
    let (whole, frac) = match digits.split_once('.') {
        Some((w, f)) => (w.to_string(), format!(".{f}")),
        None => (digits, String::new()),
    };
    let mut grouped = String::new();
    let bytes = whole.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(*b as char);
    }
    format!("{sign}{grouped}{frac}")
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        // WinAnsi covers Latin-1; everything else degrades to '?'.
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ if (c as u32) < 256 => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

fn text(ops: &mut String, size: f64, x: f64, y: f64, s: &str) {
    ops.push_str(&format!(
        "BT /F1 {:.1} Tf {:.1} {:.1} Td ({}) Tj ET\n",
        size,
        x,
        y,
        esc(s)
    ));
}

fn bold(ops: &mut String, size: f64, x: f64, y: f64, s: &str) {
    ops.push_str(&format!(
        "BT /F2 {:.1} Tf {:.1} {:.1} Td ({}) Tj ET\n",
        size,
        x,
        y,
        esc(s)
    ));
}

fn right(ops: &mut String, size: f64, right_edge: f64, y: f64, s: &str) {
    // Approximate width: 0.5 em per char is close enough for tabular use.
    let width = s.chars().count() as f64 * size * 0.5;
    text(ops, size, right_edge - width, y, s);
}

fn bold_right(ops: &mut String, size: f64, right_edge: f64, y: f64, s: &str) {
    let width = s.chars().count() as f64 * size * 0.52;
    bold(ops, size, right_edge - width, y, s);
}

fn line(ops: &mut String, x1: f64, y: f64, x2: f64) {
    ops.push_str(&format!("{:.1} {:.1} m {:.1} {:.1} l S\n", x1, y, x2, y));
}

/// Assemble the page contents into a valid PDF (catalog, pages, page,
/// contents, two base-14 fonts, xref). Byte offsets must be exact.
fn assemble_pdf(contents: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(contents.len() + 1024);
    out.extend_from_slice(b"%PDF-1.4\n");

    // Object plan: 1 catalog, 2 pages, 3 page, 4 contents, 5 F1, 6 F2.
    let mut offsets = [0usize; 7];

    push_obj(&mut out, &mut offsets, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    push_obj(
        &mut out,
        &mut offsets,
        2,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    );
    push_obj(
        &mut out,
        &mut offsets,
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
          /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> /Contents 4 0 R >>",
    );
    let stream = contents.as_bytes();
    let header = format!("<< /Length {} >>\nstream\n", stream.len());
    offsets[4] = out.len();
    out.extend_from_slice(format!("4 0 obj\n{}", header).as_bytes());
    out.extend_from_slice(stream);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    push_obj(
        &mut out,
        &mut offsets,
        5,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    push_obj(
        &mut out,
        &mut offsets,
        6,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
    );

    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for i in 1..offsets.len() {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[i]).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            offsets.len(),
            xref
        )
        .as_bytes(),
    );
    out
}

fn push_obj(out: &mut Vec<u8>, offsets: &mut [usize; 7], num: usize, body: &[u8]) {
    offsets[num] = out.len();
    out.extend_from_slice(format!("{} 0 obj\n", num).as_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendobj\n");
}

// The module doc names the file's consumers; silence the unused warning on
// the DoW helper when only the PDF is consumed.
#[allow(unused)]
fn day_of_week_label(d: chrono::NaiveDate) -> &'static str {
    match d.weekday().num_days_from_monday() {
        0 => "Mon",
        1 => "Tue",
        2 => "Wed",
        3 => "Thu",
        4 => "Fri",
        5 => "Sat",
        _ => "Sun",
    }
}
