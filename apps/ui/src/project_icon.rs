//! A project's picture from its own checkout: which files could be its icon,
//! and the images read so far, keyed by project and path.

use std::collections::{HashMap, HashSet};

use domain::{ProjectId, WorkspaceId, WorkspaceKind};

use crate::app::AppState;

/// Thumbnails the icon dialog offers.
pub const MAX_CANDIDATES: usize = 6;

/// Points an icon is rasterized for: the dialog's thumbnail, the largest
/// place a project picture draws.
pub const ICON_PT: f32 = 40.0;

/// Build output and caches: a favicon there is a copy of one in the source.
const SKIPPED_DIRS: [&str; 10] = [
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    "coverage",
    "DerivedData",
    "Pods",
];

/// Paths in a checkout listing that look like the project's icon, best
/// first: favicons, then app icons, then plain icons, then logos. One per
/// folder, so an `AppIcon.appiconset` offers its largest size rather than
/// every size it ships.
#[must_use]
pub fn candidates<'a>(entries: impl IntoIterator<Item = (&'a str, bool)>) -> Vec<String> {
    let mut ranked: Vec<(u8, usize, u8, u32, &str)> = entries
        .into_iter()
        .filter(|(path, ignored)| !ignored && domain::is_icon_image_path(path))
        .filter(|(path, _)| !path.split('/').any(|part| SKIPPED_DIRS.contains(&part)))
        .filter_map(|(path, _)| {
            let rank = rank(path)?;
            let depth = path.matches('/').count();
            Some((
                rank,
                depth,
                extension_rank(path),
                largest_number(path),
                path,
            ))
        })
        .collect();
    ranked.sort_by(|a, b| {
        (a.0, a.1, a.2)
            .cmp(&(b.0, b.1, b.2))
            .then(b.3.cmp(&a.3))
            .then(a.4.cmp(b.4))
    });
    let mut folders = HashSet::new();
    ranked
        .into_iter()
        .filter(|(.., path)| folders.insert(path.rsplit_once('/').map_or("", |(dir, _)| dir)))
        .take(MAX_CANDIDATES)
        .map(|(.., path)| path.to_string())
        .collect()
}

fn rank(path: &str) -> Option<u8> {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if stem.starts_with("favicon") {
        Some(0)
    } else if lower.contains("appicon")
        || stem.starts_with("apple-touch-icon")
        || name.ends_with(".icns")
    {
        Some(1)
    } else if stem == "icon"
        || stem.starts_with("icon-")
        || stem.starts_with("icon@")
        || stem.starts_with("icon_")
    {
        Some(2)
    } else if stem.starts_with("logo") {
        Some(3)
    } else {
        None
    }
}

/// Vector first, then lossless, then the formats that pack several sizes.
fn extension_rank(path: &str) -> u8 {
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("svg") => 0,
        Some("png") => 1,
        Some("icns") => 2,
        Some("ico") => 3,
        _ => 4,
    }
}

/// `Icon-1024.png` beats `Icon-16.png` among a folder's sizes.
fn largest_number(path: &str) -> u32 {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split(|c: char| !c.is_ascii_digit())
        .filter_map(|digits| digits.parse().ok())
        .max()
        .unwrap_or(0)
}

/// An image read for `(project, path)`: `None` while the read is in flight.
type Slot = Option<Result<rune::Image, String>>;

/// The icon dialog's suggestions for one project.
pub struct Picker {
    pub project: ProjectId,
    pub candidates: Option<Result<Vec<String>, String>>,
}

#[derive(Default)]
pub struct ProjectIcons {
    images: HashMap<(ProjectId, String), Slot>,
    pub picker: Option<Picker>,
    /// An image landed and may still be decoding.
    pending: bool,
}

impl ProjectIcons {
    /// The decoded picture for `path`, once it is ready to draw.
    #[must_use]
    pub fn image(&self, project: ProjectId, path: &str) -> Option<&rune::Image> {
        match self.images.get(&(project, path.to_string())) {
            Some(Some(Ok(image))) if image.is_ready() && image.error().is_none() => Some(image),
            _ => None,
        }
    }

    /// Whether `path` was read and could not be shown.
    #[must_use]
    pub fn failed(&self, project: ProjectId, path: &str) -> bool {
        match self.images.get(&(project, path.to_string())) {
            Some(Some(Err(_))) => true,
            Some(Some(Ok(image))) => image.error().is_some(),
            _ => false,
        }
    }

    /// Marks every wanted `(project, path)` not read yet as in flight and
    /// returns them; drops what nothing wants any more.
    pub fn take_reads(
        &mut self,
        wanted: impl IntoIterator<Item = (ProjectId, WorkspaceId, String)>,
    ) -> Vec<(ProjectId, WorkspaceId, String)> {
        let wanted: Vec<_> = wanted.into_iter().collect();
        let keep: HashSet<(ProjectId, &str)> = wanted
            .iter()
            .map(|(project, _, path)| (*project, path.as_str()))
            .collect();
        self.images
            .retain(|(project, path), _| keep.contains(&(*project, path.as_str())));
        wanted
            .into_iter()
            .filter(|(project, _, path)| {
                let key = (*project, path.clone());
                if self.images.contains_key(&key) {
                    return false;
                }
                self.images.insert(key, None);
                true
            })
            .collect()
    }

    /// An answer for a read still wanted; a late one for a dropped path is
    /// ignored. Whether anything changed on screen.
    pub fn apply(
        &mut self,
        project: ProjectId,
        path: String,
        image: Result<rune::Image, String>,
    ) -> bool {
        match self.images.get_mut(&(project, path)) {
            Some(slot) => {
                *slot = Some(image);
                self.pending = true;
                true
            }
            None => false,
        }
    }

    /// Drops `project`'s pictures, read or in flight, so the next sync reads
    /// them from disk again; `path` narrows it to one file.
    pub fn forget(&mut self, project: ProjectId, path: Option<&str>) {
        self.images.retain(|(owner, known), _| {
            *owner != project || path.is_some_and(|path| path != known)
        });
    }

    /// A reconnect drops the answers in flight; they are asked again.
    pub fn forget_reads(&mut self) {
        self.images.retain(|_, slot| slot.is_some());
    }

    /// Whether every image that landed has finished decoding since the last
    /// call, so the view is built again to draw them.
    pub fn landed(&mut self) -> bool {
        if !self.pending {
            return false;
        }
        self.pending = self
            .images
            .values()
            .any(|slot| matches!(slot, Some(Ok(image)) if !image.is_ready()));
        !self.pending
    }
}

impl AppState {
    /// Reads the pictures the rail and an open icon dialog draw. Called when
    /// the store or the dialog's suggestions change, never per frame.
    pub fn sync_project_icons(&mut self) {
        let (Some(store), Some(connection)) = (&self.store, &self.connection) else {
            return;
        };
        let main = |project: ProjectId| {
            store
                .workspaces
                .iter()
                .find(|w| w.project_id == project && w.kind == WorkspaceKind::Main)
                .map(|w| w.id)
        };
        let mut wanted: Vec<(ProjectId, WorkspaceId, String)> = store
            .projects
            .iter()
            .filter_map(|project| {
                let path = project.icon_image.clone()?;
                Some((project.id, main(project.id)?, path))
            })
            .collect();
        let open = self
            .menus
            .prompt
            .as_ref()
            .and_then(|prompt| match prompt.ask {
                crate::menus::Ask::SetIcon(project) => Some(project),
                _ => None,
            });
        if self
            .project_icons
            .picker
            .as_ref()
            .is_some_and(|picker| Some(picker.project) != open)
        {
            self.project_icons.picker = None;
        }
        if let Some(Picker {
            project,
            candidates: Some(Ok(paths)),
        }) = &self.project_icons.picker
            && let Some(workspace) = main(*project)
        {
            wanted.extend(paths.iter().map(|path| (*project, workspace, path.clone())));
        }
        for (project, workspace, path) in self.project_icons.take_reads(wanted) {
            let _ = connection.read_project_icon(self.generation, project, workspace, path);
        }
    }

    /// The project whose picture `path` of `workspace` is, when that is its
    /// main checkout; an empty path, a lost-events rescan, matches any picture.
    pub(crate) fn icon_owner(&self, workspace: WorkspaceId, path: &str) -> Option<ProjectId> {
        let store = self.store.as_ref()?;
        let checkout = store
            .workspaces
            .iter()
            .find(|w| w.id == workspace && w.kind == WorkspaceKind::Main)?;
        store
            .projects
            .iter()
            .find(|project| project.id == checkout.project_id)
            .filter(|project| {
                project
                    .icon_image
                    .as_deref()
                    .is_some_and(|icon| path.is_empty() || icon == path)
            })
            .map(|project| project.id)
    }

    /// Asks for the files of `project`'s checkout that could be its icon.
    pub fn open_icon_picker(&mut self, project: ProjectId) {
        let workspace = self.store.as_ref().and_then(|store| {
            store
                .workspaces
                .iter()
                .find(|w| w.project_id == project && w.kind == WorkspaceKind::Main)
                .map(|w| w.id)
        });
        let asked = match (&self.connection, workspace) {
            (Some(connection), Some(workspace)) => connection
                .list_icon_candidates(self.generation, project, workspace)
                .is_ok(),
            _ => false,
        };
        self.project_icons.forget(project, None);
        self.project_icons.picker = Some(Picker {
            project,
            candidates: (!asked).then(|| Ok(Vec::new())),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(paths: &[&'static str]) -> Vec<(&'static str, bool)> {
        paths.iter().map(|path| (*path, false)).collect()
    }

    #[test]
    fn favicons_come_first_and_app_icons_offer_their_largest_size() {
        let found = candidates(tracked(&[
            "README.md",
            "src/logo.svg",
            "public/favicon.ico",
            "public/favicon.svg",
            "ios/Assets.xcassets/AppIcon.appiconset/Icon-16.png",
            "ios/Assets.xcassets/AppIcon.appiconset/Icon-1024.png",
            "src-tauri/icons/icon.icns",
            "docs/screenshot.png",
        ]));
        assert_eq!(
            found,
            [
                "public/favicon.svg",
                "src-tauri/icons/icon.icns",
                "ios/Assets.xcassets/AppIcon.appiconset/Icon-1024.png",
                "src/logo.svg",
            ],
            "one per folder: the svg favicon stands for its .ico twin"
        );
    }

    #[test]
    fn build_output_and_ignored_files_are_not_offered() {
        let found = candidates([
            ("dist/favicon.ico", false),
            ("web/node_modules/pkg/favicon.png", false),
            (".cache/favicon.png", true),
            ("app/favicon.png", false),
        ]);
        assert_eq!(found, ["app/favicon.png"]);
    }

    /// Rune's decoder guesses the format from the bytes; the `ico` feature
    /// this crate turns on is what lets it read a favicon.ico.
    #[test]
    fn the_shared_decoder_reads_an_ico() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode png");
        let frame =
            image::codecs::ico::IcoFrame::with_encoded(png, 2, 2, image::ExtendedColorType::Rgba8)
                .expect("an ico frame");
        let mut ico = Vec::new();
        image::codecs::ico::IcoEncoder::new(&mut ico)
            .encode_images(&[frame])
            .expect("encode ico");
        let decoded = image::ImageReader::new(std::io::Cursor::new(ico))
            .with_guessed_format()
            .expect("guess")
            .decode()
            .expect("an ico decodes");
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    #[test]
    fn a_read_is_asked_once_and_dropped_when_nothing_wants_it() {
        let mut icons = ProjectIcons::default();
        let (project, workspace) = (ProjectId::new(), WorkspaceId::new());
        let wanted = || vec![(project, workspace, "favicon.svg".to_string())];
        assert_eq!(icons.take_reads(wanted()).len(), 1);
        assert!(icons.take_reads(wanted()).is_empty(), "in flight");
        assert!(icons.take_reads(Vec::new()).is_empty());
        assert!(
            !icons.apply(project, "favicon.svg".into(), Err("late".into())),
            "an answer nobody waits for is dropped"
        );
        assert_eq!(
            icons.take_reads(wanted()).len(),
            1,
            "asked again once wanted"
        );
        assert!(icons.apply(project, "favicon.svg".into(), Err("gone".into())));
        assert!(icons.failed(project, "favicon.svg"));
    }

    #[test]
    fn a_forgotten_picture_is_read_again() {
        let mut icons = ProjectIcons::default();
        let (project, workspace) = (ProjectId::new(), WorkspaceId::new());
        let wanted = || {
            vec![
                (project, workspace, "favicon.svg".to_string()),
                (project, workspace, "logo.png".to_string()),
            ]
        };
        assert_eq!(icons.take_reads(wanted()).len(), 2);
        assert!(icons.apply(project, "favicon.svg".into(), Err("old".into())));
        icons.forget(project, Some("favicon.svg"));
        let again = icons.take_reads(wanted());
        assert_eq!(again.len(), 1, "only the changed file");
        assert_eq!(again[0].2, "favicon.svg");
        icons.forget(project, None);
        assert_eq!(icons.take_reads(wanted()).len(), 2);
    }
}
