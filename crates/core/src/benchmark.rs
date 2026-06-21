//! Benchmark de competidores (F11). **Heurístico y determinista**: no agrega
//! endpoints; compone la metadata de videos (F9) y las keywords (F6) ya
//! ingestadas para perfilar canales y detectar brechas accionables (keywords que
//! la competencia cubre y vos no, cadencia de publicación, engagement).
//!
//! Dominio puro: `lib.rs` arma los perfiles desde lo persistido y llama acá.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::models::{Comment, VideoMeta};
use crate::text::extract_keywords;

/// Cuántas keywords top describen lo que "cubre" un canal.
const TOP_KEYWORDS: usize = 12;

/// Perfil agregado de un canal, derivado de su metadata de videos (+ comentarios
/// si los hay). Los promedios son `None` cuando ningún video informa esa métrica
/// (estadísticas ocultas): no se inventan ceros.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelProfile {
    pub channel_id: String,
    pub video_count: usize,
    pub avg_views: Option<f64>,
    pub avg_likes: Option<f64>,
    pub avg_comments: Option<f64>,
    /// Lo que el canal cubre (keywords de títulos/tags/descripciones + comentarios).
    pub top_keywords: Vec<String>,
    /// Días promedio entre publicaciones (None si hay < 2 videos).
    pub posting_cadence_days: Option<f64>,
}

/// Tipo de brecha detectada frente a un competidor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BenchmarkGapKind {
    /// Keywords que el competidor cubre y el canal propio no.
    MissingKeywords,
    /// El competidor publica más seguido.
    Cadence,
    /// El competidor tiene más engagement (vistas promedio).
    Engagement,
    /// No hay datos ingestados de ese competidor.
    NoData,
}

/// Una brecha accionable respecto de un competidor puntual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkGap {
    pub kind: BenchmarkGapKind,
    pub competitor_id: String,
    pub detail: String,
}

/// Reporte de benchmark: mi perfil, los de los competidores y las brechas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub mine: ChannelProfile,
    pub competitors: Vec<ChannelProfile>,
    pub gaps: Vec<BenchmarkGap>,
}

/// Promedio de los valores presentes (ignora `None`); `None` si no hay ninguno.
fn mean_present(values: impl Iterator<Item = Option<u64>>) -> Option<f64> {
    let present: Vec<u64> = values.flatten().collect();
    if present.is_empty() {
        None
    } else {
        Some(present.iter().sum::<u64>() as f64 / present.len() as f64)
    }
}

/// Días promedio entre publicaciones consecutivas (None si < 2 videos).
fn cadence_days(videos: &[VideoMeta]) -> Option<f64> {
    if videos.len() < 2 {
        return None;
    }
    let mut dates: Vec<_> = videos.iter().map(|v| v.published_at).collect();
    dates.sort();
    let total_secs: i64 = dates.windows(2).map(|w| (w[1] - w[0]).num_seconds()).sum();
    let gaps = (dates.len() - 1) as f64;
    Some(total_secs as f64 / 86_400.0 / gaps)
}

/// Perfila un canal a partir de su metadata de videos (F9) y, opcionalmente, sus
/// comentarios (para enriquecer las keywords). Reusa `extract_keywords` (F6).
pub fn profile_channel(
    channel_id: &str,
    videos: &[VideoMeta],
    comments: &[Comment],
) -> ChannelProfile {
    // Corpus que define lo que el canal "cubre": texto de cada video + comentarios.
    let mut docs: Vec<String> = videos
        .iter()
        .map(|v| format!("{} {} {}", v.title, v.tags.join(" "), v.description))
        .collect();
    docs.extend(comments.iter().map(|c| c.text.clone()));

    let top_keywords = extract_keywords(&docs, TOP_KEYWORDS)
        .into_iter()
        .map(|k| k.term)
        .collect();

    ChannelProfile {
        channel_id: channel_id.to_string(),
        video_count: videos.len(),
        avg_views: mean_present(videos.iter().map(|v| v.view_count)),
        avg_likes: mean_present(videos.iter().map(|v| v.like_count)),
        avg_comments: mean_present(videos.iter().map(|v| v.comment_count)),
        top_keywords,
        posting_cadence_days: cadence_days(videos),
    }
}

/// Compara mi perfil contra el de cada competidor y devuelve brechas accionables.
/// Determinista; un competidor sin datos produce una brecha `NoData` en vez de
/// fallar (estilo `incomplete`).
pub fn benchmark(mine: ChannelProfile, competitors: Vec<ChannelProfile>) -> BenchmarkReport {
    let mine_keywords: HashSet<String> = mine.top_keywords.iter().cloned().collect();
    let mut gaps = Vec::new();

    for comp in &competitors {
        if comp.video_count == 0 {
            gaps.push(BenchmarkGap {
                kind: BenchmarkGapKind::NoData,
                competitor_id: comp.channel_id.clone(),
                detail: "Sin datos ingestados de este canal: traé su metadata para compararlo."
                    .into(),
            });
            continue;
        }

        // Keywords que el competidor cubre y yo no (ordenadas, deterministas).
        let mut missing: Vec<String> = comp
            .top_keywords
            .iter()
            .filter(|k| !mine_keywords.contains(*k))
            .cloned()
            .collect();
        missing.sort();
        if !missing.is_empty() {
            gaps.push(BenchmarkGap {
                kind: BenchmarkGapKind::MissingKeywords,
                competitor_id: comp.channel_id.clone(),
                detail: format!("Cubre temas que vos no: {}.", missing.join(", ")),
            });
        }

        // Cadencia: publica más seguido (cadencia menor = más frecuente).
        if let (Some(mine_c), Some(comp_c)) = (mine.posting_cadence_days, comp.posting_cadence_days)
        {
            if comp_c < mine_c {
                gaps.push(BenchmarkGap {
                    kind: BenchmarkGapKind::Cadence,
                    competitor_id: comp.channel_id.clone(),
                    detail: format!("Publica cada {comp_c:.1} días vs tus {mine_c:.1}.",),
                });
            }
        }

        // Engagement: más vistas promedio.
        if let (Some(mine_v), Some(comp_v)) = (mine.avg_views, comp.avg_views) {
            if comp_v > mine_v {
                gaps.push(BenchmarkGap {
                    kind: BenchmarkGapKind::Engagement,
                    competitor_id: comp.channel_id.clone(),
                    detail: format!("Promedia {comp_v:.0} vistas vs tus {mine_v:.0}.",),
                });
            }
        }
    }

    BenchmarkReport {
        mine,
        competitors,
        gaps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn video(id: &str, channel: &str, day: u32, views: Option<u64>, title: &str) -> VideoMeta {
        VideoMeta {
            video_id: id.into(),
            channel_id: channel.into(),
            title: title.into(),
            description: String::new(),
            tags: vec![],
            view_count: views,
            like_count: Some(10),
            comment_count: Some(2),
            published_at: Utc
                .with_ymd_and_hms(2024, 1, day, 0, 0, 0)
                .single()
                .unwrap(),
        }
    }

    #[test]
    fn perfila_promedios_ignorando_none_y_cadencia() {
        let videos = vec![
            video("v1", "yo", 1, Some(100), "angular signals"),
            video("v2", "yo", 11, None, "angular routing"), // vistas ocultas
            video("v3", "yo", 21, Some(300), "angular forms"),
        ];
        let p = profile_channel("yo", &videos, &[]);
        assert_eq!(p.video_count, 3);
        // promedio sobre las 2 presentes: (100+300)/2 = 200.
        assert_eq!(p.avg_views, Some(200.0));
        // publicaciones día 1, 11, 21 -> 10 días entre cada una.
        assert_eq!(p.posting_cadence_days, Some(10.0));
        assert!(p.top_keywords.contains(&"angular".to_string()));
    }

    #[test]
    fn cadencia_none_con_menos_de_dos_videos() {
        let one = vec![video("v1", "yo", 1, Some(1), "uno")];
        assert_eq!(profile_channel("yo", &one, &[]).posting_cadence_days, None);
        assert_eq!(profile_channel("yo", &[], &[]).avg_views, None);
    }

    #[test]
    fn detecta_brechas_keywords_cadencia_y_engagement() {
        let mine = profile_channel(
            "yo",
            &[
                video("a", "yo", 1, Some(100), "angular basico"),
                video("b", "yo", 31, Some(100), "angular intermedio"), // cadencia 30 días
            ],
            &[],
        );
        let rival = profile_channel(
            "rival",
            &[
                video("c", "rival", 1, Some(500), "react hooks"),
                video("d", "rival", 8, Some(500), "react server components"), // cadencia 7 días
            ],
            &[],
        );
        let report = benchmark(mine, vec![rival]);
        let kinds: Vec<_> = report.gaps.iter().map(|g| g.kind).collect();
        assert!(
            kinds.contains(&BenchmarkGapKind::MissingKeywords),
            "react debería ser keyword faltante"
        );
        assert!(
            kinds.contains(&BenchmarkGapKind::Cadence),
            "el rival publica más seguido"
        );
        assert!(
            kinds.contains(&BenchmarkGapKind::Engagement),
            "el rival tiene más vistas"
        );
        // la brecha de keywords menciona "react".
        let kw = report
            .gaps
            .iter()
            .find(|g| g.kind == BenchmarkGapKind::MissingKeywords)
            .unwrap();
        assert!(kw.detail.contains("react"));
    }

    #[test]
    fn competidor_sin_datos_da_gap_nodata_sin_fallar() {
        let mine = profile_channel("yo", &[video("a", "yo", 1, Some(10), "algo")], &[]);
        let empty = profile_channel("fantasma", &[], &[]);
        let report = benchmark(mine, vec![empty]);
        assert_eq!(report.gaps.len(), 1);
        assert_eq!(report.gaps[0].kind, BenchmarkGapKind::NoData);
    }

    #[test]
    fn sin_brechas_cuando_voy_igual_o_mejor_y_es_determinista() {
        let mine = profile_channel(
            "yo",
            &[
                video("a", "yo", 1, Some(1000), "angular signals"),
                video("b", "yo", 8, Some(1000), "angular routing"), // cadencia 7
            ],
            &[],
        );
        // Mismo tema, menos vistas, publica más lento: ninguna brecha.
        let weaker = profile_channel(
            "weak",
            &[
                video("c", "weak", 1, Some(10), "angular signals"),
                video("d", "weak", 31, Some(10), "angular routing"), // cadencia 30
            ],
            &[],
        );
        let report = benchmark(mine.clone(), vec![weaker.clone()]);
        assert!(
            report.gaps.is_empty(),
            "no debería haber brechas, hubo: {:?}",
            report.gaps
        );
        // determinismo
        assert_eq!(report, benchmark(mine, vec![weaker]));
    }
}
