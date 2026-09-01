//! Reproducible release-profile benchmark for the disposable FTS5 index.

use astian_lib::search_index::{IndexDocument, SearchIndex};
use std::{fs, time::Instant};

const NOTE_COUNT: usize = 10_000;
const SEARCH_SAMPLES_PER_QUERY: usize = 200;
const INCREMENTAL_SAMPLES: usize = 200;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture_started = Instant::now();
    let mut documents = build_fixture();
    let fixture_ms = fixture_started.elapsed().as_secs_f64() * 1_000.0;
    let fixture_bytes: usize = documents
        .iter()
        .map(|document| document.searchable_content.len())
        .sum();

    let temp = tempfile::tempdir()?;
    let database_path = temp.path().join("astian-fts-benchmark.sqlite3");
    let mut index = SearchIndex::open(&database_path)?;

    let initial_started = Instant::now();
    index.rebuild(&documents)?;
    index.optimize()?;
    let initial_ms = initial_started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(index.note_count()?, NOTE_COUNT);

    let queries = [
        "kế hoạch",
        "ke hoach",
        "nghiên cứu",
        "nghien cuu",
        "dự án 4242",
        "du an 4242",
        "bảo mật ngoại tuyến",
        "bao mat ngoai tuyen",
    ];
    for query in queries {
        let results = index.search(query, 20)?;
        if results.is_empty() {
            return Err(format!("benchmark query returned no results: {query}").into());
        }
    }

    let mut search_latencies_us = Vec::with_capacity(queries.len() * SEARCH_SAMPLES_PER_QUERY);
    for query in queries {
        for _ in 0..SEARCH_SAMPLES_PER_QUERY {
            let started = Instant::now();
            let results = index.search(query, 20)?;
            search_latencies_us.push(started.elapsed().as_secs_f64() * 1_000_000.0);
            std::hint::black_box(results);
        }
    }

    let mut incremental_latencies_us = Vec::with_capacity(INCREMENTAL_SAMPLES);
    for revision in 0..INCREMENTAL_SAMPLES {
        let document = &mut documents[4_242];
        document.content_hash = format!("{revision:064x}");
        document.searchable_content.push_str(" cập nhật");
        let started = Instant::now();
        index.upsert_note(document)?;
        incremental_latencies_us.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    index.optimize()?;

    let database_bytes = fs::metadata(&database_path)?.len();
    search_latencies_us.sort_by(f64::total_cmp);
    incremental_latencies_us.sort_by(f64::total_cmp);

    println!("Astian SQLite FTS5 technical spike");
    println!("notes={NOTE_COUNT}");
    println!("fixture_utf8_bytes={fixture_bytes}");
    println!("fixture_generation_ms={fixture_ms:.2}");
    println!("database_bytes={database_bytes}");
    println!("initial_rebuild_ms={initial_ms:.2}");
    print_distribution("warm_search", &search_latencies_us);
    print_distribution("incremental_note_update", &incremental_latencies_us);
    println!("sqlite_version={}", rusqlite::version());
    println!("target={}-{}", std::env::consts::OS, std::env::consts::ARCH);
    if let Ok(processor) = std::env::var("PROCESSOR_IDENTIFIER") {
        println!("processor={processor}");
    }
    if let Ok(count) = std::thread::available_parallelism() {
        println!("logical_processors={count}");
    }

    Ok(())
}

fn build_fixture() -> Vec<IndexDocument> {
    const TOPICS: [&str; 10] = [
        "kế hoạch sản phẩm",
        "nghiên cứu người dùng",
        "bảo mật ngoại tuyến",
        "thiết kế giao diện",
        "hiệu năng tìm kiếm",
        "dữ liệu tiếng Việt",
        "quản lý dự án",
        "ghi chú cuộc họp",
        "kiến trúc phần mềm",
        "phân tích phản hồi",
    ];
    const PARAGRAPHS: [&str; 8] = [
        "Mục tiêu là giữ Markdown làm nguồn dữ liệu gốc và mọi chỉ mục đều có thể dựng lại.",
        "Nhóm thảo luận lộ trình phát triển, rủi ro kỹ thuật và các bước kiểm chứng tiếp theo.",
        "Nội dung có dấu tiếng Việt phải tìm được bằng cả truy vấn có dấu và không dấu.",
        "Liên kết [[Tài liệu tham khảo]] và #du-an giúp mô phỏng một vault được sử dụng thực tế.",
        "Kết quả tìm kiếm ưu tiên tiêu đề nhưng vẫn cần snippet từ nội dung đầy đủ.",
        "Ứng dụng hoạt động ngoại tuyến, không gửi nội dung ghi chú ra mạng và không cần tài khoản.",
        "Mỗi thay đổi chỉ cập nhật một note trong transaction thay vì index lại toàn bộ vault.",
        "Các phép đo được lặp lại trên index đã warm để tính phân vị p50, p95 và p99.",
    ];

    (0..NOTE_COUNT)
        .map(|index| {
            let topic = TOPICS[index % TOPICS.len()];
            let paragraph_count = 3 + (index * 17 % 18);
            let mut content = format!(
                "---\ntags: [du-an, tieng-viet, benchmark]\npriority: {}\n---\n\n# {} {:04}\n\nMã dự án {}.\n\n",
                index % 5,
                topic,
                index,
                index
            );
            for paragraph_index in 0..paragraph_count {
                content.push_str(PARAGRAPHS[(index + paragraph_index) % PARAGRAPHS.len()]);
                content.push_str("\n\n");
            }
            IndexDocument {
                relative_path: format!("Dự án/{:02}/{}-{:04}.md", index % 64, topic.replace(' ', "-"), index),
                display_title: format!("{topic} {index:04}"),
                content_hash: format!("{index:064x}"),
                searchable_content: content,
            }
        })
        .collect()
}

fn print_distribution(label: &str, sorted_microseconds: &[f64]) {
    println!("{label}_samples={}", sorted_microseconds.len());
    println!(
        "{label}_p50_ms={:.3}",
        percentile(sorted_microseconds, 0.50) / 1_000.0
    );
    println!(
        "{label}_p95_ms={:.3}",
        percentile(sorted_microseconds, 0.95) / 1_000.0
    );
    println!(
        "{label}_p99_ms={:.3}",
        percentile(sorted_microseconds, 0.99) / 1_000.0
    );
}

fn percentile(sorted_values: &[f64], quantile: f64) -> f64 {
    let index = ((sorted_values.len() as f64 * quantile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted_values.len().saturating_sub(1));
    sorted_values[index]
}
