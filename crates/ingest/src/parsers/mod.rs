//! 各格式 parser 实现。
//!
//! 每个实现 `DocumentParser` trait，输出统一 `Document`。
//! 新库（office_oxide / pdfsink-rs）的兼容性自测见 `tests/`。

mod csv_parser;
mod epub_parser;
mod gzip_parser;
mod image_parser;
mod json_parser;
mod office_parser;
mod pdf_parser;
mod smart_zip_parser;
mod tar_parser;
mod text_parser;
mod xlsx_parser;
mod zip_parser;

pub use csv_parser::CsvParser;
pub use epub_parser::EpubParser;
pub use gzip_parser::GzipParser;
pub use image_parser::ImageParser;
pub use json_parser::JsonParser;
pub use office_parser::OfficeParser;
pub use pdf_parser::PdfParser;
pub use smart_zip_parser::SmartZipParser;
pub use tar_parser::TarParser;
pub use text_parser::TextParser;
pub use xlsx_parser::XlsxCalamineParser;
pub use zip_parser::ZipParser;
