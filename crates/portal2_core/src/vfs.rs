//! Access to the user's Portal 2 installation.

use std::sync::Arc;

use bevy::prelude::*;
use source_formats::fs::GameFileSystem;

#[derive(Resource, Clone, Deref)]
pub struct GameFs(pub Arc<GameFileSystem>);

pub struct VfsPlugin;

impl Plugin for VfsPlugin {
    fn build(&self, app: &mut App) {
        let Some(root) = GameFileSystem::find_install() else {
            panic!(
                "Portal 2 installation not found. Install Portal 2 through Steam or set PORTAL2_DIR \
                 to the folder that contains portal2.exe."
            );
        };
        let fs = GameFileSystem::portal2(&root).expect("failed to mount Portal 2 search paths");
        info!("mounted Portal 2 from {} ({:?})", root.display(), fs.search_path_names());
        app.insert_resource(GameFs(Arc::new(fs)));
    }
}
