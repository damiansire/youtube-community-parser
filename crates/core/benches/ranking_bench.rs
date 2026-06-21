//! Bench de medición (Fase 2 / F4) — NO toca código de producción.
//!
//! Mide tiempo y memoria de `rank_commenters` / `most_active` / `least_active`
//! de `sdp-core` con datasets sintéticos de tamaño creciente, para decidir si el
//! diseño in-memory aguanta volúmenes realistas de YouTube o solo revienta en
//! escenarios extremos.
//!
//! Sin dependencias externas (no criterion, no red): timing con `Instant` y RSS
//! pico vía FFI a `GetProcessMemoryInfo` (Windows). Corre con:
//!   cargo bench -p sdp-core --bench ranking_bench
//!
//! `harness = false`: es un `main()` plano.

use std::time::Instant;

use chrono::{DateTime, TimeZone, Utc};
use sdp_core::{least_active, models::Comment, models::Commenter, most_active, rank_commenters};

// ---------------------------------------------------------------------------
// Medición de memoria del proceso (Windows, sin crates externos)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod mem {
    // PROCESS_MEMORY_COUNTERS: solo necesitamos WorkingSetSize y PeakWorkingSetSize.
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    extern "system" {
        fn GetCurrentProcess() -> isize;
        // K32GetProcessMemoryInfo está en kernel32 (no requiere psapi.lib aparte).
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    /// (working_set_actual, working_set_pico) en bytes.
    pub fn rss() -> (usize, usize) {
        unsafe {
            let mut c = ProcessMemoryCounters {
                cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
                ..Default::default()
            };
            let ok = K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut c,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            );
            if ok != 0 {
                (c.working_set_size, c.peak_working_set_size)
            } else {
                (0, 0)
            }
        }
    }
}

#[cfg(not(windows))]
mod mem {
    pub fn rss() -> (usize, usize) {
        (0, 0)
    }
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

// ---------------------------------------------------------------------------
// Generación de datasets sintéticos
// ---------------------------------------------------------------------------

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap()
}

/// Genera `n` comentarios repartidos entre `unique` autores distintos.
/// `unique` modela cuánta gente realmente comenta (la cardinalidad importa
/// para el sort: el sort es O(unique log unique), la agregación O(n)).
fn make_dataset(n: usize, unique: usize) -> (Vec<Comment>, Vec<Commenter>) {
    let unique = unique.max(1).min(n.max(1));
    let mut comments = Vec::with_capacity(n);
    for i in 0..n {
        let author = i % unique;
        comments.push(Comment {
            id: format!("c{i}"),
            video_id: format!("vid{}", i % 200), // ~200 videos en un canal
            author_channel_id: format!("UC_author_{author:08}"),
            text: "Comentario sintético de ejemplo para medir el ranking.".to_string(),
            like_count: (i % 37) as u64,
            published_at: at(1_600_000_000 + i as i64),
        });
    }
    let commenters = (0..unique)
        .map(|a| Commenter {
            channel_id: format!("UC_author_{a:08}"),
            display_name: format!("Persona {a}"),
            profile_image_url: None,
            channel_url: None,
        })
        .collect();
    (comments, commenters)
}

/// Tamaño aproximado en RAM de los comentarios (estructura + heap de strings).
fn approx_comments_bytes(comments: &[Comment]) -> usize {
    comments
        .iter()
        .map(|c| {
            std::mem::size_of::<Comment>()
                + c.id.capacity()
                + c.video_id.capacity()
                + c.author_channel_id.capacity()
                + c.text.capacity()
        })
        .sum()
}

fn time_ms<F: FnMut()>(mut f: F) -> f64 {
    let t = Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

fn run_for(n: usize, unique: usize) {
    let (comments, commenters) = make_dataset(n, unique);
    let input_mb = mb(approx_comments_bytes(&comments));

    let (rss_before, _) = mem::rss();

    // rank_commenters (O(n) agregación + O(unique log unique) sort)
    let mut out_len = 0usize;
    let t_rank = time_ms(|| {
        let r = rank_commenters(&comments, &commenters);
        out_len = r.len();
        std::hint::black_box(&r);
    });

    let t_most = time_ms(|| {
        let r = most_active(&comments, &commenters, 10);
        std::hint::black_box(&r);
    });

    let t_least = time_ms(|| {
        let r = least_active(&comments, &commenters, 10);
        std::hint::black_box(&r);
    });

    let (rss_after, rss_peak) = mem::rss();
    let delta_mb = mb(rss_after.saturating_sub(rss_before));

    println!(
        "N={:>7} | autores únicos={:>7} | input≈{:>8.2} MB | rank={:>8.3} ms | most_active(10)={:>8.3} ms | least_active(10)={:>8.3} ms | stats out={:>7} | ΔRSS≈{:>7.2} MB | RSS pico={:>7.2} MB",
        n, out_len, input_mb, t_rank, t_most, t_least, out_len, delta_mb, mb(rss_peak)
    );
}

fn main() {
    println!("== Medición Fase 2 (F4) — ranking in-memory de sdp-core ==");
    println!(
        "Plataforma: {} | perfil release (cargo bench)",
        std::env::consts::OS
    );
    println!("Nota: 'most_active' y 'least_active' recalculan rank_commenters internamente (F5: ranking x3).\n");

    // Caso A: muchos comentarios, audiencia mediana (cardinalidad ~10% de N).
    println!("--- Caso A: autores únicos ≈ 10% de N (audiencia mediana) ---");
    for &n in &[1_000usize, 10_000, 100_000, 500_000] {
        run_for(n, (n / 10).max(1));
    }

    // Caso B: peor caso para el sort — casi todos los comentarios de autores
    // distintos (cardinalidad ~ N). Estresa el O(unique log unique).
    println!("\n--- Caso B: autores únicos ≈ N (peor caso del sort) ---");
    for &n in &[1_000usize, 10_000, 100_000, 500_000] {
        run_for(n, n);
    }

    // Caso C: audiencia chica/fiel (pocos autores, muchos comentarios c/u).
    println!("\n--- Caso C: pocos autores (audiencia chica y fiel) ---");
    for &n in &[10_000usize, 100_000, 500_000] {
        run_for(n, 100);
    }
}
