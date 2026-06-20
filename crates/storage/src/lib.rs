//! Persistencia local en SQLite del histórico de comentarios y comentaristas.
//!
//! Guarda lo que ingiere el sidecar para poder analizar la evolución de la
//! comunidad en el tiempo, sin volver a pegarle a la API. El dominio
//! (`sdp-core`) sigue siendo puro: acá vive el efecto de I/O.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use sdp_core::{Comment, Commenter};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("fecha inválida en la base: {0}")]
    Date(#[from] chrono::ParseError),
}

type Result<T> = std::result::Result<T, StoreError>;

/// Conexión a la base local.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Abre (o crea) la base en disco y asegura el esquema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_conn(Connection::open(path)?)
    }

    /// Base en memoria, para tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commenter (
                channel_id        TEXT PRIMARY KEY,
                display_name      TEXT NOT NULL,
                profile_image_url TEXT,
                channel_url       TEXT
            );
            CREATE TABLE IF NOT EXISTS comment (
                id                TEXT PRIMARY KEY,
                video_id          TEXT NOT NULL,
                author_channel_id TEXT NOT NULL,
                text              TEXT NOT NULL,
                like_count        INTEGER NOT NULL,
                published_at      TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_comment_author
                ON comment(author_channel_id);",
        )?;
        Ok(Store { conn })
    }

    /// Inserta o actualiza comentaristas (idempotente por `channel_id`).
    pub fn save_commenters(&mut self, commenters: &[Commenter]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO commenter (channel_id, display_name, profile_image_url, channel_url)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(channel_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    profile_image_url = excluded.profile_image_url,
                    channel_url = excluded.channel_url",
            )?;
            for c in commenters {
                stmt.execute(params![
                    c.channel_id,
                    c.display_name,
                    c.profile_image_url,
                    c.channel_url
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Inserta o actualiza comentarios (idempotente por `id`).
    pub fn save_comments(&mut self, comments: &[Comment]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO comment (id, video_id, author_channel_id, text, like_count, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    text = excluded.text,
                    like_count = excluded.like_count,
                    published_at = excluded.published_at",
            )?;
            for c in comments {
                stmt.execute(params![
                    c.id,
                    c.video_id,
                    c.author_channel_id,
                    c.text,
                    c.like_count,
                    c.published_at.to_rfc3339(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Todos los comentaristas guardados.
    pub fn all_commenters(&self) -> Result<Vec<Commenter>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, display_name, profile_image_url, channel_url FROM commenter",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Commenter {
                channel_id: r.get(0)?,
                display_name: r.get(1)?,
                profile_image_url: r.get(2)?,
                channel_url: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Todos los comentarios guardados.
    pub fn all_comments(&self) -> Result<Vec<Comment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, video_id, author_channel_id, text, like_count, published_at FROM comment",
        )?;
        let rows = stmt.query_map([], |r| {
            let published: String = r.get(5)?;
            Ok((
                Comment {
                    id: r.get(0)?,
                    video_id: r.get(1)?,
                    author_channel_id: r.get(2)?,
                    text: r.get(3)?,
                    like_count: r.get(4)?,
                    // se valida al parsear, fuera del closure de rusqlite
                    published_at: Utc::now(),
                },
                published,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (mut comment, published) = row?;
            comment.published_at = DateTime::parse_from_rfc3339(&published)?.with_timezone(&Utc);
            out.push(comment);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn commenter(id: &str, name: &str) -> Commenter {
        Commenter {
            channel_id: id.into(),
            display_name: name.into(),
            profile_image_url: None,
            channel_url: None,
        }
    }

    fn comment(id: &str, author: &str, likes: u64, h: u32) -> Comment {
        Comment {
            id: id.into(),
            video_id: "v1".into(),
            author_channel_id: author.into(),
            text: "hola".into(),
            like_count: likes,
            published_at: Utc.with_ymd_and_hms(2021, 9, 27, h, 0, 0).single().unwrap(),
        }
    }

    #[test]
    fn guarda_y_recupera_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        store.save_commenters(&[commenter("ana", "Ana")]).unwrap();
        store.save_comments(&[comment("c1", "ana", 5, 10)]).unwrap();

        let commenters = store.all_commenters().unwrap();
        assert_eq!(commenters.len(), 1);
        assert_eq!(commenters[0].display_name, "Ana");

        let comments = store.all_comments().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].like_count, 5);
        assert_eq!(
            comments[0].published_at,
            comment("c1", "ana", 5, 10).published_at
        );
    }

    #[test]
    fn upsert_es_idempotente_y_actualiza() {
        let mut store = Store::open_in_memory().unwrap();
        store.save_comments(&[comment("c1", "ana", 1, 10)]).unwrap();
        // mismo id, más likes: debe actualizar, no duplicar.
        store
            .save_comments(&[comment("c1", "ana", 99, 10)])
            .unwrap();

        let comments = store.all_comments().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].like_count, 99);
    }

    #[test]
    fn integra_con_los_rankings_del_core() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .save_commenters(&[commenter("ana", "Ana"), commenter("beto", "Beto")])
            .unwrap();
        store
            .save_comments(&[
                comment("c1", "ana", 0, 10),
                comment("c2", "ana", 0, 11),
                comment("c3", "beto", 0, 12),
            ])
            .unwrap();

        let ranking = sdp_core::rank_commenters(
            &store.all_comments().unwrap(),
            &store.all_commenters().unwrap(),
        );
        assert_eq!(ranking[0].channel_id, "ana");
        assert_eq!(ranking[0].comment_count, 2);
    }
}
