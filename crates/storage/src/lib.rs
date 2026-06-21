//! Persistencia local en SQLite del histórico de comentarios y comentaristas.
//!
//! Guarda lo que ingiere el sidecar para poder analizar la evolución de la
//! comunidad en el tiempo, sin volver a pegarle a la API. El dominio
//! (`sdp-core`) sigue siendo puro: acá vive el efecto de I/O.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use sdp_core::{Comment, Commenter, VideoMeta};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("fecha inválida en la base: {0}")]
    Date(#[from] chrono::ParseError),
    #[error("tags inválidos en la base (JSON): {0}")]
    Json(#[from] serde_json::Error),
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
        // PRAGMAs de rendimiento (auditoría P9): WAL deja que lecturas y escrituras
        // no se bloqueen entre sí (en rollback-journal cada escritura hacía fsync y
        // trababa las lecturas); `synchronous=NORMAL` es seguro bajo WAL y evita un
        // fsync por transacción. Se corren una sola vez al abrir, junto con el DDL.
        //
        // En bases `:memory:` el WAL no aplica (no hay archivo), pero el pragma es
        // inocuo: SQLite lo ignora y queda en `memory`.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
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
                ON comment(author_channel_id);
            CREATE TABLE IF NOT EXISTS video_meta (
                video_id      TEXT PRIMARY KEY,
                channel_id    TEXT NOT NULL,
                title         TEXT NOT NULL,
                description   TEXT NOT NULL,
                tags          TEXT NOT NULL,
                view_count    INTEGER,
                like_count    INTEGER,
                comment_count INTEGER,
                published_at  TEXT NOT NULL
            );",
        )?;
        Ok(Store { conn })
    }

    /// Modo de journal activo (`"wal"`, `"memory"`, …). Se expone para verificar
    /// en tests que `open` activó WAL en una base de archivo (auditoría P9).
    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?)
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
        // Traemos las columnas crudas (incluida la fecha como string) y armamos el
        // `Comment` recién tras parsear la fecha fuera del closure de rusqlite,
        // igual que `all_video_meta`. Así no hace falta un `Utc::now()` placeholder
        // muerto que luego se pisa (auditoría P14: era impuro y, si un refactor
        // olvidaba la reasignación, dejaba la fecha en "ahora" en silencio).
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (id, video_id, author_channel_id, text, like_count, published) = row?;
            out.push(Comment {
                id,
                video_id,
                author_channel_id,
                text,
                like_count,
                published_at: DateTime::parse_from_rfc3339(&published)?.with_timezone(&Utc),
            });
        }
        Ok(out)
    }

    /// Inserta o actualiza metadata de videos (F9, idempotente por `video_id`).
    /// Los `tags` se guardan como JSON en una columna TEXT; las cuentas pueden
    /// ser `NULL` (estadísticas ocultas).
    pub fn save_video_meta(&mut self, videos: &[VideoMeta]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO video_meta
                    (video_id, channel_id, title, description, tags,
                     view_count, like_count, comment_count, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(video_id) DO UPDATE SET
                    channel_id = excluded.channel_id,
                    title = excluded.title,
                    description = excluded.description,
                    tags = excluded.tags,
                    view_count = excluded.view_count,
                    like_count = excluded.like_count,
                    comment_count = excluded.comment_count,
                    published_at = excluded.published_at",
            )?;
            for v in videos {
                let tags = serde_json::to_string(&v.tags)?;
                stmt.execute(params![
                    v.video_id,
                    v.channel_id,
                    v.title,
                    v.description,
                    tags,
                    v.view_count.map(|n| n as i64),
                    v.like_count.map(|n| n as i64),
                    v.comment_count.map(|n| n as i64),
                    v.published_at.to_rfc3339(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Toda la metadata de videos guardada.
    pub fn all_video_meta(&self) -> Result<Vec<VideoMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, channel_id, title, description, tags,
                    view_count, like_count, comment_count, published_at
             FROM video_meta",
        )?;
        // Traemos las columnas crudas (tags JSON + fecha string) y las parseamos
        // fuera del closure de rusqlite, igual que en `all_comments`.
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, String>(8)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (video_id, channel_id, title, description, tags, views, likes, comments, published) =
                row?;
            out.push(VideoMeta {
                video_id,
                channel_id,
                title,
                description,
                tags: serde_json::from_str(&tags)?,
                view_count: views.map(|n| n as u64),
                like_count: likes.map(|n| n as u64),
                comment_count: comments.map(|n| n as u64),
                published_at: DateTime::parse_from_rfc3339(&published)?.with_timezone(&Utc),
            });
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

    fn video_meta(id: &str, views: Option<u64>) -> VideoMeta {
        VideoMeta {
            video_id: id.into(),
            channel_id: "UCcanal".into(),
            title: format!("titulo {id}"),
            description: "desc".into(),
            tags: vec!["angular".into(), "signals".into()],
            view_count: views,
            like_count: Some(56),
            comment_count: Some(7),
            published_at: Utc.with_ymd_and_hms(2021, 9, 27, 3, 0, 0).single().unwrap(),
        }
    }

    #[test]
    fn video_meta_round_trip_con_stats_ocultas() {
        let mut store = Store::open_in_memory().unwrap();
        // un video con stats visibles y otro con view_count oculto (None).
        store
            .save_video_meta(&[video_meta("v1", Some(1234)), video_meta("v2", None)])
            .unwrap();

        let mut metas = store.all_video_meta().unwrap();
        metas.sort_by(|a, b| a.video_id.cmp(&b.video_id));
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].view_count, Some(1234));
        assert_eq!(metas[0].tags, vec!["angular", "signals"]);
        assert_eq!(metas[1].view_count, None, "None se preserva como NULL");
        assert_eq!(metas[1].like_count, Some(56));
    }

    #[test]
    fn video_meta_upsert_es_idempotente() {
        let mut store = Store::open_in_memory().unwrap();
        store.save_video_meta(&[video_meta("v1", Some(1))]).unwrap();
        store
            .save_video_meta(&[video_meta("v1", Some(999))])
            .unwrap();

        let metas = store.all_video_meta().unwrap();
        assert_eq!(metas.len(), 1, "mismo id no duplica");
        assert_eq!(metas[0].view_count, Some(999), "actualiza el valor");
    }

    #[test]
    fn open_en_archivo_activa_wal() {
        // P9: una base de archivo debe quedar en WAL (lecturas/escrituras sin
        // bloquearse mutuamente). Usamos un archivo temporal único y lo limpiamos.
        let mut path = std::env::temp_dir();
        let unique = format!(
            "sdp-wal-test-{}-{:?}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        path.push(unique);

        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.journal_mode().unwrap().to_lowercase(),
            "wal",
            "open() en archivo debe activar WAL"
        );
        drop(store);

        // Limpieza best-effort del .sqlite3 y sus sidecars -wal/-shm.
        let _ = std::fs::remove_file(&path);
        for suffix in ["-wal", "-shm"] {
            let mut side = path.clone();
            let name = format!("{}{suffix}", path.file_name().unwrap().to_string_lossy());
            side.set_file_name(name);
            let _ = std::fs::remove_file(side);
        }
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
