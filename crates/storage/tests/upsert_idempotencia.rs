//! Tests de integración del upsert idempotente del `Store`, vía la API pública
//! del crate. Respaldan el claim del README: cada análisis se guarda con
//! "idempotent upsert" (re-guardar lo mismo no duplica; re-guardar con datos
//! nuevos actualiza). La migración de esquema y los datos corruptos se cubren
//! en los tests unitarios de `src/lib.rs`.

use chrono::{TimeZone, Utc};
use sdp_core::{Comment, Commenter, VideoMeta};
use sdp_storage::Store;

fn commenter(id: &str, name: &str) -> Commenter {
    Commenter {
        channel_id: id.into(),
        display_name: name.into(),
        profile_image_url: None,
        channel_url: None,
    }
}

fn comment(id: &str, author: &str, likes: u64) -> Comment {
    Comment {
        id: id.into(),
        video_id: "v1".into(),
        author_channel_id: author.into(),
        text: "hola".into(),
        like_count: likes,
        published_at: Utc
            .with_ymd_and_hms(2021, 9, 27, 10, 0, 0)
            .single()
            .unwrap(),
    }
}

fn video_meta(id: &str, views: Option<u64>) -> VideoMeta {
    VideoMeta {
        video_id: id.into(),
        channel_id: "UCcanal".into(),
        title: format!("titulo {id}"),
        description: "desc".into(),
        tags: vec!["angular".into()],
        view_count: views,
        like_count: Some(56),
        comment_count: Some(7),
        published_at: Utc.with_ymd_and_hms(2021, 9, 27, 3, 0, 0).single().unwrap(),
    }
}

#[test]
fn upsert_de_comentarios_no_duplica_y_actualiza() {
    let mut store = Store::open_in_memory().unwrap();
    store.save_comments(&[comment("c1", "ana", 1)]).unwrap();
    // mismo id, mas likes: debe actualizar la fila, no crear otra.
    store.save_comments(&[comment("c1", "ana", 99)]).unwrap();

    let comments = store.all_comments().unwrap();
    assert_eq!(comments.len(), 1, "mismo id no duplica");
    assert_eq!(comments[0].like_count, 99, "actualiza el valor");
}

#[test]
fn upsert_de_comentaristas_no_duplica_y_actualiza() {
    let mut store = Store::open_in_memory().unwrap();
    store.save_commenters(&[commenter("ana", "Ana")]).unwrap();
    store
        .save_commenters(&[commenter("ana", "Ana Maria")])
        .unwrap();

    let commenters = store.all_commenters().unwrap();
    assert_eq!(commenters.len(), 1, "mismo channel_id no duplica");
    assert_eq!(commenters[0].display_name, "Ana Maria");
}

#[test]
fn upsert_de_video_meta_no_duplica_y_actualiza() {
    let mut store = Store::open_in_memory().unwrap();
    store.save_video_meta(&[video_meta("v1", Some(1))]).unwrap();
    store
        .save_video_meta(&[video_meta("v1", Some(999))])
        .unwrap();

    let metas = store.all_video_meta().unwrap();
    assert_eq!(metas.len(), 1, "mismo video_id no duplica");
    assert_eq!(metas[0].view_count, Some(999), "actualiza el valor");
}

#[test]
fn re_guardar_el_mismo_lote_deja_la_base_identica() {
    // El caso "analyze de nuevo sin gastar cuota": correr el mismo guardado dos
    // veces produce exactamente el mismo estado que correrlo una vez.
    let mut store = Store::open_in_memory().unwrap();
    let lote = [comment("c1", "ana", 5), comment("c2", "beto", 3)];

    store.save_comments(&lote).unwrap();
    store.save_comments(&lote).unwrap();

    let mut comments = store.all_comments().unwrap();
    comments.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].id, "c1");
    assert_eq!(comments[1].id, "c2");
}
