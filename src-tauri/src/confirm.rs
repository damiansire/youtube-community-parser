//! Gate de confirmación con **token de un solo uso** (auditoría P1).
//!
//! El gate de dinero/cuota era evadible: `run_*` aceptaba un `confirmed: bool`
//! crudo del front, así que cualquier caller IPC (o un front comprometido con
//! `withGlobalTauri: true`) pasaba `confirmed: true` y gastaba plata/cuota sin
//! ver jamás el modal.
//!
//! Acá el contrato es **estimar → confirmar → ejecutar** ligado por un token:
//! - `estimate_*` calcula el costo y emite un `confirmation_token` (nonce) ligado
//!   a un **fingerprint** (tipo de operación + monto exacto + hash del corpus que
//!   se va a procesar);
//! - el front muestra el modal y, al confirmar, llama a `run_*` devolviendo ese
//!   token;
//! - `run_*` **consume** el token y verifica que su fingerprint coincide con el
//!   re-calculado server-side **en el momento de ejecutar**. Si el corpus cambió
//!   entre estimar y ejecutar (TOCTOU, auditoría idx 13), el hash no coincide y
//!   se rechaza: lo confirmado == lo ejecutado.
//!
//! Las operaciones gratis (`requires_confirmation = false`) no necesitan token.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use sdp_core::CostEstimate;

/// Huella de una operación a confirmar: la liga al costo exacto y al snapshot de
/// datos que va a procesar. Dos operaciones con el mismo fingerprint son, a
/// efectos del gate, la misma decisión del usuario.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpFingerprint {
    /// Identifica la operación (p. ej. `"refine_ideas_ai:Anthropic"`).
    pub op: String,
    /// El `CostKind` serializado de forma estable (incluye monto/unidades).
    pub cost: String,
    /// Hash del corpus/insumo que la operación procesará (0 si no aplica).
    pub corpus_hash: u64,
}

impl OpFingerprint {
    /// Construye un fingerprint a partir de la operación, su estimate y un hash
    /// del insumo. El costo se toma del `kind` re-estimado server-side.
    pub fn new(op: impl Into<String>, estimate: &CostEstimate, corpus_hash: u64) -> Self {
        Self {
            op: op.into(),
            cost: format!("{:?}", estimate.kind),
            corpus_hash,
        }
    }
}

/// Hash determinista y estable de un corpus de textos (orden-sensible, como lo
/// procesa la operación). Sirve para detectar que el insumo no cambió entre
/// estimar y ejecutar.
pub fn hash_texts<S: AsRef<str>>(texts: &[S]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    texts.len().hash(&mut h);
    for t in texts {
        t.as_ref().hash(&mut h);
    }
    h.finish()
}

/// Almacén de tokens de confirmación pendientes (vive en `tauri::State`).
///
/// Cada token es de **un solo uso**: `consume` lo saca del mapa, así que un
/// replay del mismo token (o un caller que se inventa uno) falla.
#[derive(Default)]
pub struct ConfirmStore {
    pending: Mutex<HashMap<String, OpFingerprint>>,
    seq: Mutex<u64>,
}

/// Por qué falló la validación de un token de confirmación.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmError {
    /// No se mandó token para una operación que sí requiere confirmación.
    Missing,
    /// El token no existe (ya se usó, expiró o es inventado).
    Unknown,
    /// El token existe pero su fingerprint no coincide con el re-calculado
    /// server-side (cambió el monto o el corpus entre estimar y ejecutar).
    Mismatch,
}

impl ConfirmStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emite un token nuevo para `fingerprint` y lo registra como pendiente.
    /// Devuelve el token opaco que el front debe devolver al confirmar.
    pub fn issue(&self, fingerprint: OpFingerprint) -> String {
        let token = {
            let mut seq = self.seq.lock().expect("lock seq");
            *seq += 1;
            // Token opaco: secuencia + hash del fingerprint. No revela datos.
            let mut h = std::collections::hash_map::DefaultHasher::new();
            fingerprint.hash(&mut h);
            seq.hash(&mut h);
            format!("ct_{:016x}{:08x}", h.finish(), *seq)
        };
        self.pending
            .lock()
            .expect("lock pending")
            .insert(token.clone(), fingerprint);
        token
    }

    /// Valida y **consume** un token: debe existir y su fingerprint debe coincidir
    /// con `expected` (re-calculado server-side al ejecutar). Un token válido se
    /// elimina (un solo uso) aunque el fingerprint no matchee, para que no quede
    /// colgado.
    pub fn consume(
        &self,
        token: Option<&str>,
        expected: &OpFingerprint,
    ) -> Result<(), ConfirmError> {
        let token = token.ok_or(ConfirmError::Missing)?;
        let mut pending = self.pending.lock().expect("lock pending");
        match pending.remove(token) {
            None => Err(ConfirmError::Unknown),
            Some(fp) if &fp == expected => Ok(()),
            Some(_) => Err(ConfirmError::Mismatch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdp_core::{CostKind, CostLine};

    fn est_money(usd: u64) -> CostEstimate {
        let kind = CostKind::Money { usd_micros: usd };
        CostEstimate {
            kind,
            requires_confirmation: true,
            breakdown: vec![CostLine {
                label: "x".into(),
                kind,
            }],
        }
    }

    #[test]
    fn token_valido_se_consume_una_sola_vez() {
        let store = ConfirmStore::new();
        let fp = OpFingerprint::new("op", &est_money(1000), 42);
        let token = store.issue(fp.clone());
        // Primer uso: ok.
        assert_eq!(store.consume(Some(&token), &fp), Ok(()));
        // Segundo uso (replay): el token ya no existe.
        assert_eq!(store.consume(Some(&token), &fp), Err(ConfirmError::Unknown));
    }

    #[test]
    fn sin_token_es_missing() {
        let store = ConfirmStore::new();
        let fp = OpFingerprint::new("op", &est_money(1000), 1);
        assert_eq!(store.consume(None, &fp), Err(ConfirmError::Missing));
    }

    #[test]
    fn token_inventado_es_unknown() {
        let store = ConfirmStore::new();
        let fp = OpFingerprint::new("op", &est_money(1000), 1);
        assert_eq!(
            store.consume(Some("ct_inventado"), &fp),
            Err(ConfirmError::Unknown)
        );
    }

    #[test]
    fn token_de_otro_monto_no_sirve() {
        // El usuario confirmó US$0.001 pero al ejecutar el costo es otro.
        let store = ConfirmStore::new();
        let confirmed = OpFingerprint::new("op", &est_money(1000), 7);
        let token = store.issue(confirmed);
        let at_run = OpFingerprint::new("op", &est_money(9999), 7);
        assert_eq!(
            store.consume(Some(&token), &at_run),
            Err(ConfirmError::Mismatch)
        );
    }

    #[test]
    fn token_con_corpus_cambiado_no_sirve_toctou() {
        // Confirmó sobre un corpus; al ejecutar el corpus cambió (más comentarios).
        let store = ConfirmStore::new();
        let est = est_money(1000);
        let confirmed = OpFingerprint::new("op", &est, hash_texts(&["a", "b"]));
        let token = store.issue(confirmed);
        let at_run = OpFingerprint::new("op", &est, hash_texts(&["a", "b", "c"]));
        assert_eq!(
            store.consume(Some(&token), &at_run),
            Err(ConfirmError::Mismatch)
        );
    }

    #[test]
    fn hash_textos_es_orden_sensible_y_estable() {
        assert_eq!(hash_texts(&["a", "b"]), hash_texts(&["a", "b"]));
        assert_ne!(hash_texts(&["a", "b"]), hash_texts(&["b", "a"]));
        assert_ne!(hash_texts(&["a"]), hash_texts(&["a", ""]));
    }

    #[test]
    fn tokens_distintos_por_emision() {
        let store = ConfirmStore::new();
        let fp = OpFingerprint::new("op", &est_money(1), 1);
        assert_ne!(store.issue(fp.clone()), store.issue(fp));
    }
}
