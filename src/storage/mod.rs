mod error;
mod home;
mod index_file;
mod symlink;
mod tracks;
mod workspace;

pub use error::StorageError;
pub use home::{
    index_path, resolve_gts_home, storage_root, track_git_dir, workspace_storage_dir, HomeOptions,
};
pub use index_file::{read_index, write_index, IndexData, WorkspaceEntry};
pub use symlink::{replace_git_symlink, replace_git_symlink_to_track, resolve_active_track};
pub use tracks::{
    assert_destination_available, assert_track_name_available, ensure_track_dir, list_tracks,
    validate_track_name,
};
pub use workspace::{
    find_workspace_by_path, find_workspace_root_from, register_workspace, RegisteredWorkspace,
    WorkspaceRoot,
};
