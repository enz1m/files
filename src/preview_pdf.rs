// Read-only Poppler previews, matching Reader's renderer. Native objects never
// cross the blocking worker; each request retains one bounded page raster.
use enzui_core::decode_limits::DecodeLimits;
use std::{io::Read, path::Path, sync::Mutex};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
static POPPLER: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Page {
    pub(crate) index: u32,
    pub(crate) count: u32,
    pub(crate) image: iced::widget::image::Handle,
}

pub(crate) fn render(path: &Path, index: u32) -> Result<Page, String> {
    let file = crate::services::open_regular_file(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES {
        return Err(crate::i18n::tr("PDF exceeds the 64 MiB preview limit").into());
    }
    if !meta.is_file() {
        return Err(crate::i18n::tr("Choose a regular PDF file").into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(crate::i18n::tr("PDF exceeds the 64 MiB preview limit").into());
    }
    let _lock = POPPLER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let doc = poppler::Document::from_bytes(&glib::Bytes::from_owned(bytes), None)
        .map_err(|e| e.to_string())?;
    let count = doc.n_pages().max(0) as u32;
    if index >= count {
        return Err(crate::i18n::tr("PDF page is unavailable").into());
    }
    let page = doc
        .page(index as i32)
        .ok_or_else(|| crate::i18n::tr("PDF page is unavailable").to_owned())?;
    let (w, h) = page.size();
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 || w > 1e7 || h > 1e7 {
        return Err(crate::i18n::tr("Invalid PDF page dimensions").into());
    }
    let target = enzui_services::document_preview::plan_raster_target(
        1000,
        (w * 1024.0).round().max(1.0) as u64,
        (h * 1024.0).round().max(1.0) as u64,
        &DecodeLimits::DEFAULT,
    )
    .map_err(|e| e.to_string())?;
    let width = target.width();
    let height = target.height();
    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, width as i32, height as i32)
            .map_err(|e| e.to_string())?;
    {
        let ctx = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
        ctx.set_source_rgb(1.0, 1.0, 1.0);
        ctx.paint().map_err(|e| e.to_string())?;
        ctx.scale(f64::from(width) / w, f64::from(height) / h);
        page.render(&ctx);
    }
    let stride = surface.stride() as usize;
    let data = surface.data().map_err(|e| e.to_string())?;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        for pixel in data[y * stride..y * stride + width as usize * 4]
            .as_chunks::<4>()
            .0
        {
            let argb = u32::from_ne_bytes(*pixel);
            rgba.extend_from_slice(&[(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, 255]);
        }
    }
    Ok(Page {
        index,
        count,
        image: iced::widget::image::Handle::from_rgba(width, height, rgba),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn stream(contents: &str) -> String {
        format!(
            "<< /Length {} >>\nstream\n{contents}endstream",
            contents.len()
        )
    }

    fn two_pages(path: &Path) {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 140] /Contents 5 0 R >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 140 100] /Contents 6 0 R >>".to_owned(),
            stream("1 0 0 rg 0 0 100 140 re f\n"),
            stream("0 0 1 rg 0 0 140 100 re f\n"),
        ];
        let mut data = String::from("%PDF-1.4\n");
        let mut offsets = vec![0];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(data.len());
            write!(data, "{} 0 obj\n{object}\nendobj\n", i + 1).unwrap();
        }
        let xref = data.len();
        data.push_str("xref\n0 7\n0000000000 65535 f \n");
        for offset in &offsets[1..] {
            writeln!(data, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            data,
            "trailer << /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .unwrap();
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn pdf_renders_distinct_pages_and_rejects_invalid_or_oversized_sources() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("notes.pdf");
        two_pages(&path);
        let first = render(&path, 0).unwrap();
        let second = render(&path, 1).unwrap();
        assert_eq!((first.index, second.index, second.count), (0, 1, 2));
        match (&first.image, &second.image) {
            (
                iced::widget::image::Handle::Rgba { pixels: a, .. },
                iced::widget::image::Handle::Rgba { pixels: b, .. },
            ) => {
                assert!(a.as_chunks::<4>().0.iter().any(|p| p[0] > 200 && p[2] < 10));
                assert!(b.as_chunks::<4>().0.iter().any(|p| p[2] > 200 && p[0] < 10));
            }
            _ => panic!("PDF preview must provide decoded pixels"),
        }
        // Real asynchronous navigation advances once and cannot recreate a closed preview.
        let (mut app, _) = crate::App::new();
        let id = iced::window::Id::unique();
        app.previews.insert(
            id,
            crate::preview::Preview {
                path: path.clone(),
                name: "notes.pdf".into(),
                type_label: "PDF".into(),
                size_label: String::new(),
                content: crate::preview::PreviewContent::Pdf {
                    page: first,
                    pending: None,
                    error: None,
                },
                width: 720.0,
            },
        );
        crate::file_clipboard_tests::dispatch(&mut app, crate::Msg::PreviewPdfPage(id, 1));
        assert!(
            matches!(&app.previews[&id].content, crate::preview::PreviewContent::Pdf {page, pending: None, ..} if page.index == 1)
        );
        let task = app.update(crate::Msg::PreviewPdfPage(id, 0));
        app.previews.remove(&id);
        crate::file_clipboard_tests::drive(&mut app, task);
        assert!(!app.previews.contains_key(&id));
        assert!(render(&path, 2).is_err());
        std::fs::write(&path, b"not a PDF").unwrap();
        assert!(render(&path, 0).is_err());
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(render(&path, 0).unwrap_err().contains("64 MiB"));
    }
}
