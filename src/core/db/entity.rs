use sea_orm::entity::prelude::*;
use serde::{
    Deserialize,
    Serialize,
};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "lyrics")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    #[sea_orm(unique, indexed)]
    pub filename: String,
    pub timestamp: i64,

    pub track_names: Json,
    pub artist_names: Json,
    pub album_names: Json,

    pub normalized_track_names: Json,
    pub normalized_artist_names: Json,
    pub normalized_album_names: Json,

    pub ncm_music_ids: Json,
    pub qq_music_ids: Json,
    pub apple_music_ids: Json,
    pub spotify_ids: Json,

    pub isrcs: Json,

    pub author_ids: Json,
    pub author_usernames: Json,

    #[sea_orm(column_type = "Text")]
    pub lyric_text: String,
    #[sea_orm(column_type = "Text")]
    pub bg_vocal_text: String,
    #[sea_orm(column_type = "Text")]
    pub raw_ttml: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub mod meta {
    use sea_orm::entity::prelude::*;
    use serde::{
        Deserialize,
        Serialize,
    };

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "meta_info")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        pub value: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
