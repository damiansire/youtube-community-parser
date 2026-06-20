//! Rankings de comentaristas: el corazón de "conocer a tus usuarios".
//!
//! A partir de una lista de comentarios calcula, por persona, cuánto participa
//! (cantidad, likes, primera y última vez que comentó) y los ordena para
//! responder *quiénes son los que más comentan y los que menos*.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{ChannelId, Comment, Commenter};

/// Estadísticas agregadas de una persona dentro del conjunto analizado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommenterStats {
    pub channel_id: ChannelId,
    /// Nombre visible si lo conocemos (puede faltar si no vino el perfil).
    pub display_name: Option<String>,
    /// Cantidad total de comentarios.
    pub comment_count: u64,
    /// Suma de likes recibidos en todos sus comentarios.
    pub total_likes: u64,
    /// Primer comentario observado.
    pub first_seen: DateTime<Utc>,
    /// Último comentario observado.
    pub last_seen: DateTime<Utc>,
}

/// Calcula las estadísticas por comentarista y las devuelve ordenadas de
/// **más** a **menos** activo.
///
/// El desempate es determinista (más likes, luego `channel_id`) para que el
/// orden sea estable y testeable. `commenters` aporta los nombres visibles; si
/// una persona comentó pero no está en `commenters`, igual aparece (sin nombre).
pub fn rank_commenters(comments: &[Comment], commenters: &[Commenter]) -> Vec<CommenterStats> {
    let names: HashMap<&ChannelId, &str> = commenters
        .iter()
        .map(|c| (&c.channel_id, c.display_name.as_str()))
        .collect();

    let mut by_person: HashMap<&ChannelId, CommenterStats> = HashMap::new();

    for c in comments {
        by_person
            .entry(&c.author_channel_id)
            .and_modify(|s| {
                s.comment_count += 1;
                s.total_likes += c.like_count;
                s.first_seen = s.first_seen.min(c.published_at);
                s.last_seen = s.last_seen.max(c.published_at);
            })
            .or_insert_with(|| CommenterStats {
                channel_id: c.author_channel_id.clone(),
                display_name: names.get(&c.author_channel_id).map(|n| n.to_string()),
                comment_count: 1,
                total_likes: c.like_count,
                first_seen: c.published_at,
                last_seen: c.published_at,
            });
    }

    let mut stats: Vec<CommenterStats> = by_person.into_values().collect();
    sort_most_active_first(&mut stats);
    stats
}

/// Ordena in-place de más a menos activo, con desempate determinista.
fn sort_most_active_first(stats: &mut [CommenterStats]) {
    stats.sort_by(|a, b| {
        b.comment_count
            .cmp(&a.comment_count)
            .then(b.total_likes.cmp(&a.total_likes))
            .then(a.channel_id.cmp(&b.channel_id))
    });
}

/// Los `n` que **más** comentan.
pub fn most_active(comments: &[Comment], commenters: &[Commenter], n: usize) -> Vec<CommenterStats> {
    let mut ranked = rank_commenters(comments, commenters);
    ranked.truncate(n);
    ranked
}

/// Los `n` que **menos** comentan (el extremo inferior del mismo ranking).
pub fn least_active(comments: &[Comment], commenters: &[Commenter], n: usize) -> Vec<CommenterStats> {
    let ranked = rank_commenters(comments, commenters);
    let start = ranked.len().saturating_sub(n);
    ranked[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    fn commenter(id: &str, name: &str) -> Commenter {
        Commenter {
            channel_id: id.into(),
            display_name: name.into(),
            profile_image_url: None,
            channel_url: None,
        }
    }

    fn comment(id: &str, author: &str, likes: u64, t: i64) -> Comment {
        Comment {
            id: id.into(),
            video_id: "vid1".into(),
            author_channel_id: author.into(),
            text: "hola".into(),
            like_count: likes,
            published_at: at(t),
        }
    }

    fn sample() -> (Vec<Comment>, Vec<Commenter>) {
        // ana: 3 comentarios | beto: 2 | caro: 1
        let comments = vec![
            comment("c1", "ana", 5, 100),
            comment("c2", "ana", 0, 200),
            comment("c3", "ana", 1, 300),
            comment("c4", "beto", 2, 150),
            comment("c5", "beto", 2, 250),
            comment("c6", "caro", 9, 120),
        ];
        let commenters = vec![
            commenter("ana", "Ana"),
            commenter("beto", "Beto"),
            commenter("caro", "Caro"),
        ];
        (comments, commenters)
    }

    #[test]
    fn cuenta_comentarios_y_likes_por_persona() {
        let (comments, commenters) = sample();
        let ranked = rank_commenters(&comments, &commenters);

        let ana = ranked.iter().find(|s| s.channel_id == "ana").unwrap();
        assert_eq!(ana.comment_count, 3);
        assert_eq!(ana.total_likes, 6);
        assert_eq!(ana.display_name.as_deref(), Some("Ana"));
        assert_eq!(ana.first_seen, at(100));
        assert_eq!(ana.last_seen, at(300));
    }

    #[test]
    fn ordena_de_mas_a_menos_activo() {
        let (comments, commenters) = sample();
        let ranked = rank_commenters(&comments, &commenters);
        let orden: Vec<&str> = ranked.iter().map(|s| s.channel_id.as_str()).collect();
        assert_eq!(orden, vec!["ana", "beto", "caro"]);
    }

    #[test]
    fn mas_activos_devuelve_el_tope() {
        let (comments, commenters) = sample();
        let top = most_active(&comments, &commenters, 2);
        let ids: Vec<&str> = top.iter().map(|s| s.channel_id.as_str()).collect();
        assert_eq!(ids, vec!["ana", "beto"]);
    }

    #[test]
    fn menos_activos_devuelve_la_cola() {
        let (comments, commenters) = sample();
        let bottom = least_active(&comments, &commenters, 2);
        let ids: Vec<&str> = bottom.iter().map(|s| s.channel_id.as_str()).collect();
        assert_eq!(ids, vec!["beto", "caro"]);
    }

    #[test]
    fn desempate_por_likes_es_determinista() {
        // dos personas con 1 comentario: gana la de más likes.
        let comments = vec![comment("c1", "x", 1, 10), comment("c2", "y", 50, 10)];
        let ranked = rank_commenters(&comments, &[]);
        let ids: Vec<&str> = ranked.iter().map(|s| s.channel_id.as_str()).collect();
        assert_eq!(ids, vec!["y", "x"]);
    }

    #[test]
    fn comentarista_sin_perfil_aparece_sin_nombre() {
        let comments = vec![comment("c1", "fantasma", 0, 10)];
        let ranked = rank_commenters(&comments, &[]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].channel_id, "fantasma");
        assert_eq!(ranked[0].display_name, None);
    }

    #[test]
    fn sin_comentarios_devuelve_vacio() {
        assert!(rank_commenters(&[], &[]).is_empty());
        assert!(most_active(&[], &[], 5).is_empty());
        assert!(least_active(&[], &[], 5).is_empty());
    }
}
