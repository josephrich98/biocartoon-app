//! PowerPoint (.pptx) export: a deck with a slide per exported slide, each item on it a shape of
//! its own, so it can still be moved, resized, rotated or deleted on its own in PowerPoint. Text
//! boxes are PowerPoint text boxes (or pictures, if asked); everything else is a picture.
//! Pictures carry an SVG (PowerPoint 2016 and later draw it as vectors) and a PNG that older
//! versions and other apps (Keynote, Google Slides) show instead. Items that share a group become
//! a PowerPoint group. [`crate::export`] renders the pictures; this writes the package.

use crate::offline::ZipWriter;
use std::fmt::Write as _;

/// English Metric Units per document pixel (96 per inch).
const EMU_PER_PX: f64 = 914_400.0 / crate::doc::DPI as f64;
/// PowerPoint's limits on the slide size: 1 to 56 inches a side.
const MIN_SIDE: f64 = 914_400.0;
const MAX_SIDE: f64 = 51_206_400.0;

pub struct Deck {
    pub title: String,
    /// Slide width and height in document pixels.
    pub size: [f32; 2],
    pub slides: Vec<SlideOut>,
}

pub struct SlideOut {
    pub background: [u8; 4],
    /// Back to front.
    pub shapes: Vec<Shape>,
}

pub enum Shape {
    Picture(Picture),
    Text(TextBox),
}

impl Shape {
    fn frame(&self) -> ([f32; 2], [f32; 2], Option<u64>) {
        match self {
            Shape::Picture(p) => (p.pos, p.size, p.group),
            Shape::Text(t) => (t.pos, t.size, t.group),
        }
    }
}

/// An editable text box: its text in paragraphs of runs, the text wrapping at its width and free
/// to run past its bottom, as on the canvas.
pub struct TextBox {
    pub name: String,
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub rotation: f32,
    pub group: Option<u64>,
    /// The box's own colour behind the text (alpha 0: none).
    pub fill: [u8; 4],
    /// "l", "ctr", "r" or "just".
    pub align: &'static str,
    /// "t", "ctr" or "b".
    pub anchor: &'static str,
    /// Multiple of single spacing.
    pub line_spacing: f32,
    pub paragraphs: Vec<Vec<Run>>,
    /// Size and typeface for an empty paragraph's line.
    pub size_px: f32,
    pub typeface: String,
}

pub struct Run {
    pub text: String,
    pub size_px: f32,
    pub typeface: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: [u8; 4],
    /// Superscript (above 0) or subscript (below 0), in thousandths of a percent of the size.
    pub baseline: i32,
    pub highlight: Option<[u8; 4]>,
}

pub struct Picture {
    pub name: String,
    /// Alt text: the artwork's credits.
    pub descr: String,
    /// Where the picture goes before rotation, in document pixels from the slide's top left.
    pub pos: [f32; 2],
    pub size: [f32; 2],
    /// Clockwise, in radians, about the picture's centre.
    pub rotation: f32,
    /// 0..1, for pictures whose PNG doesn't already have it.
    pub alpha: f32,
    pub png: Vec<u8>,
    pub svg: Option<Vec<u8>>,
    pub group: Option<u64>,
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const EMPTY_TREE: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

/// Text for an XML attribute or element: escaped, without the control characters XML forbids.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

fn hex(c: [u8; 4]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// `<a:srgbClr>` with its alpha, if any.
fn color(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!(r#"<a:srgbClr val="{}"/>"#, hex(c))
    } else {
        format!(r#"<a:srgbClr val="{}"><a:alpha val="{}"/></a:srgbClr>"#, hex(c), (c[3] as u32 * 100_000 + 127) / 255)
    }
}

/// `rot="..."` for a clockwise turn of `radians`, or nothing.
fn rot_attr(radians: f32) -> String {
    let rot = (radians.to_degrees().rem_euclid(360.0) as f64 * 60_000.0).round() as i64 % 21_600_000;
    if rot != 0 { format!(r#" rot="{rot}""#) } else { String::new() }
}

/// A text box's `<p:sp>`, its sizes in hundredths of a point from `pt_per_px`.
fn text_box(t: &TextBox, id: usize, emu: &dyn Fn(f32) -> i64, pt_per_px: f64) -> String {
    let sz = |px: f32| ((px as f64 * pt_per_px * 100.0).round() as i64).clamp(100, 400_000);
    let font = |face: &str| {
        let face = esc(face);
        format!(r#"<a:latin typeface="{face}"/><a:ea typeface="{face}"/><a:cs typeface="{face}"/>"#)
    };
    let fill = if t.fill[3] > 0 { format!("<a:solidFill>{}</a:solidFill>", color(t.fill)) } else { "<a:noFill/>".into() };
    let mut body = String::new();
    let spacing = (t.line_spacing.max(0.1) as f64 * 100_000.0).round() as i64;
    for para in &t.paragraphs {
        let _ = write!(body, r#"<a:p><a:pPr algn="{}"><a:lnSpc><a:spcPct val="{spacing}"/></a:lnSpc></a:pPr>"#, t.align);
        for r in para {
            let mut attrs = format!(r#" lang="en-US" sz="{}" b="{}" i="{}" dirty="0""#, sz(r.size_px), r.bold as u8, r.italic as u8);
            if r.underline {
                attrs.push_str(r#" u="sng""#);
            }
            if r.strike {
                attrs.push_str(r#" strike="sngStrike""#);
            }
            if r.baseline != 0 {
                let _ = write!(attrs, r#" baseline="{}""#, r.baseline);
            }
            let highlight = r.highlight.map(|h| format!("<a:highlight>{}</a:highlight>", color([h[0], h[1], h[2], 255]))).unwrap_or_default();
            let _ = write!(body, "<a:r><a:rPr{attrs}><a:solidFill>{}</a:solidFill>{highlight}{}</a:rPr><a:t>{}</a:t></a:r>", color(r.color), font(&r.typeface), esc(&r.text));
        }
        let _ = write!(body, r#"<a:endParaRPr lang="en-US" sz="{}" dirty="0">{}</a:endParaRPr></a:p>"#, sz(t.size_px), font(&t.typeface));
    }
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm{}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{fill}</p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" tIns="0" rIns="0" bIns="0" anchor="{}" rtlCol="0"><a:noAutofit/></a:bodyPr><a:lstStyle/>{body}</p:txBody></p:sp>"#,
        esc(if t.name.is_empty() { "Text" } else { &t.name }),
        rot_attr(t.rotation),
        emu(t.pos[0]),
        emu(t.pos[1]),
        emu(t.size[0]).max(1),
        emu(t.size[1]).max(1),
        t.anchor,
    )
}

fn rels(list: &[(String, &str, String)]) -> String {
    let mut s = format!(r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (id, kind, target) in list {
        let ty = if kind.starts_with("http") { kind.to_string() } else { format!("{REL}/{kind}") };
        let _ = write!(s, r#"<Relationship Id="{id}" Type="{ty}" Target="{}"/>"#, esc(target));
    }
    s.push_str("</Relationships>");
    s
}

/// The whole deck as .pptx bytes.
pub fn write(deck: &Deck) -> Result<Vec<u8>, String> {
    // Fit the slide within PowerPoint's limits, scaling everything on it alike.
    let (w, h) = (deck.size[0] as f64 * EMU_PER_PX, deck.size[1] as f64 * EMU_PER_PX);
    let scale = if w.max(h) > MAX_SIDE {
        MAX_SIDE / w.max(h)
    } else if w.min(h) < MIN_SIDE {
        (MIN_SIDE / w.min(h).max(1.0)).min(MAX_SIDE / w.max(h).max(1.0))
    } else {
        1.0
    } * EMU_PER_PX;
    let emu = |px: f32| (px as f64 * scale).round() as i64;
    let (cx, cy) = (emu(deck.size[0]).clamp(MIN_SIDE as i64, MAX_SIDE as i64), emu(deck.size[1]).clamp(MIN_SIDE as i64, MAX_SIDE as i64));

    let mut zip = ZipWriter::default();
    let mut bytes = Vec::new();
    let mut add = |zip: &mut ZipWriter, name: &str, data: &[u8]| -> Result<(), String> {
        // PNGs are compressed already.
        let part = if name.ends_with(".png") {
            let mut v = zip.entry(name, data)?;
            v.extend_from_slice(data);
            v
        } else {
            zip.deflated(name, data)?
        };
        bytes.extend_from_slice(&part);
        Ok(())
    };

    let n = deck.slides.len();
    let mut types = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="svg" ContentType="image/svg+xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/presProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presProps+xml"/><Override PartName="/ppt/viewProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml"/><Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#
    );
    for i in 1..=n {
        let _ = write!(types, r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#);
    }
    types.push_str("</Types>");
    add(&mut zip, "[Content_Types].xml", types.as_bytes())?;

    add(
        &mut zip,
        "_rels/.rels",
        rels(&[
            ("rId1".into(), "officeDocument", "ppt/presentation.xml".into()),
            ("rId2".into(), "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties", "docProps/core.xml".into()),
            ("rId3".into(), "extended-properties", "docProps/app.xml".into()),
        ])
        .as_bytes(),
    )?;
    let core = format!(
        r#"{XML}<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title></cp:coreProperties>"#,
        esc(&deck.title)
    );
    add(&mut zip, "docProps/core.xml", core.as_bytes())?;
    let app = format!(r#"{XML}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>BioCartoon</Application><Slides>{n}</Slides></Properties>"#);
    add(&mut zip, "docProps/app.xml", app.as_bytes())?;

    // The presentation: master, theme and props, then the slides.
    let mut pres_rels = vec![
        ("rId1".to_string(), "slideMaster", "slideMasters/slideMaster1.xml".to_string()),
        ("rId2".into(), "theme", "theme/theme1.xml".into()),
        ("rId3".into(), "presProps", "presProps.xml".into()),
        ("rId4".into(), "viewProps", "viewProps.xml".into()),
        ("rId5".into(), "tableStyles", "tableStyles.xml".into()),
    ];
    let mut ids = String::new();
    for i in 1..=n {
        pres_rels.push((format!("rId{}", i + 5), "slide", format!("slides/slide{i}.xml")));
        let _ = write!(ids, r#"<p:sldId id="{}" r:id="rId{}"/>"#, 255 + i, i + 5);
    }
    let pres = format!(
        r#"{XML}<p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="{cx}" cy="{cy}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    );
    add(&mut zip, "ppt/presentation.xml", pres.as_bytes())?;
    add(&mut zip, "ppt/_rels/presentation.xml.rels", rels(&pres_rels).as_bytes())?;
    add(&mut zip, "ppt/presProps.xml", format!(r#"{XML}<p:presentationPr {NS}/>"#).as_bytes())?;
    add(&mut zip, "ppt/viewProps.xml", format!(r#"{XML}<p:viewPr {NS}><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#).as_bytes())?;
    add(&mut zip, "ppt/tableStyles.xml", format!(r#"{XML}<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#).as_bytes())?;

    let master = format!(
        r#"{XML}<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle/><p:bodyStyle/><p:otherStyle/></p:txStyles></p:sldMaster>"#
    );
    add(&mut zip, "ppt/slideMasters/slideMaster1.xml", master.as_bytes())?;
    add(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        rels(&[("rId1".into(), "slideLayout", "../slideLayouts/slideLayout1.xml".into()), ("rId2".into(), "theme", "../theme/theme1.xml".into())]).as_bytes(),
    )?;
    let layout = format!(r#"{XML}<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#);
    add(&mut zip, "ppt/slideLayouts/slideLayout1.xml", layout.as_bytes())?;
    add(&mut zip, "ppt/slideLayouts/_rels/slideLayout1.xml.rels", rels(&[("rId1".into(), "slideMaster", "../slideMasters/slideMaster1.xml".into())]).as_bytes())?;
    add(&mut zip, "ppt/theme/theme1.xml", theme().as_bytes())?;

    let mut media = 0;
    for (i, slide) in deck.slides.iter().enumerate() {
        let mut slide_rels = vec![("rId1".to_string(), "slideLayout", "../slideLayouts/slideLayout1.xml".to_string())];
        let mut tree = String::new();
        let mut shape_id = 1;
        let mut k = 0;
        while k < slide.shapes.len() {
            // A run of shapes in one group becomes a group shape.
            let group = slide.shapes[k].frame().2;
            let mut end = k + 1;
            if group.is_some() {
                while end < slide.shapes.len() && slide.shapes[end].frame().2 == group {
                    end += 1;
                }
            }
            let run = &slide.shapes[k..end];
            let grouped = run.len() > 1;
            if grouped {
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for s in run {
                    let (pos, size, _) = s.frame();
                    x0 = x0.min(pos[0]);
                    y0 = y0.min(pos[1]);
                    x1 = x1.max(pos[0] + size[0]);
                    y1 = y1.max(pos[1] + size[1]);
                }
                shape_id += 1;
                let (x, y, w, h) = (emu(x0), emu(y0), emu(x1 - x0).max(1), emu(y1 - y0).max(1));
                let _ = write!(
                    tree,
                    r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="{shape_id}" name="Group {shape_id}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/><a:chOff x="{x}" y="{y}"/><a:chExt cx="{w}" cy="{h}"/></a:xfrm></p:grpSpPr>"#
                );
            }
            for s in run {
                shape_id += 1;
                let p = match s {
                    Shape::Text(t) => {
                        tree.push_str(&text_box(t, shape_id, &emu, scale / 12_700.0));
                        continue;
                    }
                    Shape::Picture(p) => p,
                };
                media += 1;
                let png_id = format!("rId{}", slide_rels.len() + 1);
                slide_rels.push((png_id.clone(), "image", format!("../media/image{media}.png")));
                add(&mut zip, &format!("ppt/media/image{media}.png"), &p.png)?;
                let mut blip_ext = String::new();
                if let Some(svg) = &p.svg {
                    let svg_id = format!("rId{}", slide_rels.len() + 1);
                    slide_rels.push((svg_id.clone(), "image", format!("../media/image{media}.svg")));
                    add(&mut zip, &format!("ppt/media/image{media}.svg"), svg)?;
                    blip_ext = format!(
                        r#"<a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="{svg_id}"/></a:ext></a:extLst>"#
                    );
                }
                let alpha = if p.alpha < 0.999 { format!(r#"<a:alphaModFix amt="{}"/>"#, (p.alpha.clamp(0.0, 1.0) * 100_000.0).round() as i64) } else { String::new() };
                let rot = rot_attr(p.rotation);
                let _ = write!(
                    tree,
                    r#"<p:pic><p:nvPicPr><p:cNvPr id="{shape_id}" name="{}" descr="{}"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{png_id}">{alpha}{blip_ext}</a:blip><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm{rot}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
                    esc(if p.name.is_empty() { "Picture" } else { &p.name }),
                    esc(p.descr.trim()),
                    emu(p.pos[0]),
                    emu(p.pos[1]),
                    emu(p.size[0]).max(1),
                    emu(p.size[1]).max(1)
                );
            }
            if grouped {
                tree.push_str("</p:grpSp>");
            }
            k = end;
        }
        let bg = if slide.background[3] > 0 {
            format!(r#"<p:bg><p:bgPr><a:solidFill><a:srgbClr val="{}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>"#, hex(slide.background))
        } else {
            String::new()
        };
        let xml = format!(r#"{XML}<p:sld {NS}><p:cSld>{bg}<p:spTree>{EMPTY_TREE}{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#);
        add(&mut zip, &format!("ppt/slides/slide{}.xml", i + 1), xml.as_bytes())?;
        add(&mut zip, &format!("ppt/slides/_rels/slide{}.xml.rels", i + 1), rels(&slide_rels).as_bytes())?;
    }
    bytes.extend(zip.finish());
    Ok(bytes)
}

/// A plain Office theme; the master needs one.
fn theme() -> String {
    let fills = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#.repeat(3);
    let lines = r#"<a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#.repeat(3);
    let effects = "<a:effectStyle><a:effectLst/></a:effectStyle>".repeat(3);
    let font = |face: &str| format!(r#"<a:latin typeface="{face}"/><a:ea typeface=""/><a:cs typeface=""/>"#);
    let clr = |name: &str, v: &str| format!(r#"<a:{name}><a:srgbClr val="{v}"/></a:{name}>"#);
    format!(
        r#"{XML}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="BioCartoon"><a:themeElements><a:clrScheme name="BioCartoon"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>{}{}{}{}{}{}{}{}{}{}</a:clrScheme><a:fontScheme name="BioCartoon"><a:majorFont>{}</a:majorFont><a:minorFont>{}</a:minorFont></a:fontScheme><a:fmtScheme name="BioCartoon"><a:fillStyleLst>{fills}</a:fillStyleLst><a:lnStyleLst>{lines}</a:lnStyleLst><a:effectStyleLst>{effects}</a:effectStyleLst><a:bgFillStyleLst>{fills}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#,
        clr("dk2", "44546A"),
        clr("lt2", "E7E6E6"),
        clr("accent1", "4472C4"),
        clr("accent2", "ED7D31"),
        clr("accent3", "A5A5A5"),
        clr("accent4", "FFC000"),
        clr("accent5", "5B9BD5"),
        clr("accent6", "70AD47"),
        clr("hlink", "0563C1"),
        clr("folHlink", "954F72"),
        font("Calibri Light"),
        font("Calibri"),
    )
}
