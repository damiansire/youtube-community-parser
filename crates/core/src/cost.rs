//! Gate de costo (principio transversal del roadmap, diseñado en F6).
//!
//! Contrato **estimar → confirmar → ejecutar**: este módulo es la parte
//! *estimar*, y es **dominio puro** (sin red ni IPC). Calcula cuánto costaría una
//! operación —en unidades de cuota de la YouTube Data API v3 o en dinero real
//! (IA, F12)— para que la UI lo muestre **antes** de ejecutar y el usuario
//! confirme. `lib.rs` re-calcula el estimate server-side antes de cada `run_*`
//! (no confía en el front); acá vive sólo el cálculo.
//!
//! La cuota es entera y finita (10k u/día); el dinero usa `usd_micros` (millonésimas
//! de dólar) para no arrastrar floats.

use serde::{Deserialize, Serialize};

// --- Tabla de cuota de la YouTube Data API v3 (unidades por request) ---------
// https://developers.google.com/youtube/v3/determine_quota_cost

/// `search.list` — la operación cara (100 u por página).
pub const SEARCH_LIST_UNITS: u32 = 100;
/// `videos.list` — barata (1 u por request de hasta 50 ids).
pub const VIDEOS_LIST_UNITS: u32 = 1;
/// `commentThreads.list` — 1 u por página.
pub const COMMENT_THREADS_LIST_UNITS: u32 = 1;
/// `playlistItems.list` — 1 u por página.
pub const PLAYLIST_ITEMS_LIST_UNITS: u32 = 1;

/// Máximo de ids que acepta `videos.list` en un request.
const VIDEOS_LIST_MAX_IDS: usize = 50;

/// Naturaleza del costo de una operación. Separa la **cuota** de YouTube (entera,
/// finita) del **dinero** real (llamadas a IA en F12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CostKind {
    /// Unidades de cuota de la Data API v3.
    QuotaUnits(u32),
    /// Dinero real, en millonésimas de dólar (evita floats).
    Money { usd_micros: u64 },
}

impl CostKind {
    /// `true` si no hay costo (0 unidades / 0 dinero): operación gratis.
    pub fn is_free(&self) -> bool {
        matches!(
            self,
            CostKind::QuotaUnits(0) | CostKind::Money { usd_micros: 0 }
        )
    }
}

/// Una línea del desglose, para que la UI muestre "videos.list ×2: 2u".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostLine {
    /// Texto legible de la operación (p. ej. `"search.list ×1"`).
    pub label: String,
    /// Costo de esta línea.
    pub kind: CostKind,
}

/// Estimación completa de una operación: el total (`kind`), su desglose y si
/// requiere confirmación explícita del usuario antes de ejecutar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Costo total de la operación.
    pub kind: CostKind,
    /// Desglose por sub-operación (para mostrar en el modal).
    pub breakdown: Vec<CostLine>,
    /// Si la UI debe pedir confirmación (modal) antes de ejecutar. Calculado con
    /// la política por defecto; `needs_optin` permite re-evaluar con otra.
    pub requires_confirmation: bool,
}

/// Política de opt-in configurable. Toda operación con `Money` siempre pide
/// confirmación; la cuota la pide si **supera** el umbral.
///
/// El `Default` deja el umbral en `0`: cualquier gasto de cuota (>0) pide opt-in
/// y sólo lo gratis (F6–F8) auto-confirma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CostPolicy {
    /// Unidades de cuota toleradas sin confirmar.
    pub quota_optin_threshold: u32,
}

/// ¿Este costo requiere opt-in bajo la política dada? Dinero → siempre; cuota →
/// sólo si supera el umbral.
fn kind_needs_optin(kind: &CostKind, policy: &CostPolicy) -> bool {
    match kind {
        CostKind::Money { usd_micros } => *usd_micros > 0,
        CostKind::QuotaUnits(units) => *units > policy.quota_optin_threshold,
    }
}

/// ¿La estimación requiere confirmación explícita bajo `policy`? (todo `Money`
/// con costo > 0 y toda cuota sobre el umbral). Es el chequeo que `lib.rs` corre
/// server-side antes de habilitar un `run_*`.
pub fn needs_optin(estimate: &CostEstimate, policy: &CostPolicy) -> bool {
    kind_needs_optin(&estimate.kind, policy)
}

/// Construye una estimación de una sola línea con `requires_confirmation`
/// calculado bajo la política por defecto.
fn single(label: String, units: u32) -> CostEstimate {
    let kind = CostKind::QuotaUnits(units);
    CostEstimate {
        kind,
        requires_confirmation: kind_needs_optin(&kind, &CostPolicy::default()),
        breakdown: vec![CostLine { label, kind }],
    }
}

/// Cuántos requests necesita `videos.list` para `n` ids (chunks de 50).
fn videos_requests(n_ids: usize) -> u32 {
    n_ids.div_ceil(VIDEOS_LIST_MAX_IDS) as u32
}

/// Estimación de traer metadata de `n_ids` videos (F9, `videos.list`): 1 unidad
/// por cada chunk de hasta 50 ids. `0` ids → gratis.
pub fn estimate_video_meta(n_ids: usize) -> CostEstimate {
    let requests = videos_requests(n_ids);
    let units = requests * VIDEOS_LIST_UNITS;
    single(format!("videos.list ×{requests}"), units)
}

/// Estimación de una búsqueda (F10, `search.list`): 100 unidades por página.
/// `max_pages = 0` → gratis (no se pega a la red).
pub fn estimate_search(max_pages: u32) -> CostEstimate {
    let units = max_pages * SEARCH_LIST_UNITS;
    single(format!("search.list ×{max_pages}"), units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_meta_chunkea_la_cuota_de_a_50() {
        assert_eq!(estimate_video_meta(0).kind, CostKind::QuotaUnits(0));
        assert_eq!(estimate_video_meta(1).kind, CostKind::QuotaUnits(1));
        assert_eq!(estimate_video_meta(50).kind, CostKind::QuotaUnits(1));
        assert_eq!(estimate_video_meta(51).kind, CostKind::QuotaUnits(2));
        assert_eq!(estimate_video_meta(100).kind, CostKind::QuotaUnits(2));
        assert_eq!(estimate_video_meta(101).kind, CostKind::QuotaUnits(3));
    }

    #[test]
    fn search_cuesta_100_por_pagina() {
        assert_eq!(estimate_search(1).kind, CostKind::QuotaUnits(100));
        assert_eq!(estimate_search(3).kind, CostKind::QuotaUnits(300));
        assert_eq!(estimate_search(0).kind, CostKind::QuotaUnits(0));
    }

    #[test]
    fn breakdown_legible_para_la_ui() {
        let est = estimate_search(2);
        assert_eq!(est.breakdown.len(), 1);
        assert_eq!(est.breakdown[0].label, "search.list ×2");
        assert_eq!(est.breakdown[0].kind, CostKind::QuotaUnits(200));
        let v = estimate_video_meta(60);
        assert_eq!(v.breakdown[0].label, "videos.list ×2");
    }

    #[test]
    fn gratis_no_requiere_confirmacion() {
        // F6–F8 / casos de 0 unidades: auto-confirmable.
        assert!(!estimate_video_meta(0).requires_confirmation);
        assert!(!estimate_search(0).requires_confirmation);
        assert!(CostKind::QuotaUnits(0).is_free());
    }

    #[test]
    fn cualquier_cuota_positiva_pide_optin_por_defecto() {
        // Incluso videos.list (1u) estrena el modal en F9.
        assert!(estimate_video_meta(1).requires_confirmation);
        assert!(estimate_search(1).requires_confirmation);
    }

    #[test]
    fn needs_optin_respeta_un_umbral_configurable() {
        let policy = CostPolicy {
            quota_optin_threshold: 100,
        };
        // 1u (videos) por debajo del umbral 100 → no pide opt-in con esta política.
        assert!(!needs_optin(&estimate_video_meta(1), &policy));
        // 100u (1 página de search) NO supera 100 → no pide; 200u sí.
        assert!(!needs_optin(&estimate_search(1), &policy));
        assert!(needs_optin(&estimate_search(2), &policy));
    }

    #[test]
    fn money_siempre_pide_optin() {
        let est = CostEstimate {
            kind: CostKind::Money { usd_micros: 12_000 },
            breakdown: vec![],
            requires_confirmation: true,
        };
        // Aun con un umbral de cuota altísimo, el dinero siempre confirma.
        let lax = CostPolicy {
            quota_optin_threshold: u32::MAX,
        };
        assert!(needs_optin(&est, &lax));
        // Money de 0 no es opt-in (no hay costo real).
        assert!(!kind_needs_optin(&CostKind::Money { usd_micros: 0 }, &lax));
        assert!(CostKind::Money { usd_micros: 0 }.is_free());
    }
}
