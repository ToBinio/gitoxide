#[cfg(unix)]
use std::os::unix::prelude::MetadataExt;
use std::{
    fs,
    io::{ErrorKind, ErrorKind::AlreadyExists},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use gix_features::progress;
use gix_object::bstr::ByteSlice;
use gix_odb::FindExt;
use gix_worktree::checkout::Collision;
use tempfile::TempDir;

use crate::fixture_path;

#[test]
fn accidental_writes_through_symlinks_are_prevented_if_overwriting_is_forbidden() {
    let mut opts = opts_from_probe();
    // without overwrite mode, everything is safe.
    opts.overwrite_existing = false;
    let (source_tree, destination, _index, outcome) =
        checkout_index_in_tmp_dir(opts.clone(), "make_dangerous_symlink").unwrap();

    let source_files = dir_structure(&source_tree);
    let worktree_files = dir_structure(&destination);

    if opts.fs.ignore_case {
        assert_eq!(
            stripped_prefix(&source_tree, &source_files),
            stripped_prefix(&destination, &worktree_files),
        );
        if multi_threaded() {
            assert_eq!(outcome.collisions.len(), 2);
        } else {
            assert_eq!(
                outcome.collisions,
                vec![
                    Collision {
                        path: "FAKE-DIR".into(),
                        error_kind: AlreadyExists
                    },
                    Collision {
                        path: "FAKE-FILE".into(),
                        error_kind: AlreadyExists
                    }
                ]
            );
        }
    } else {
        let expected = ["A-dir/a", "A-file", "FAKE-DIR", "FAKE-FILE", "fake-dir/b", "fake-file"];
        assert_eq!(stripped_prefix(&source_tree, &source_files), paths(expected));
        assert_eq!(stripped_prefix(&destination, &worktree_files), paths(expected));
        assert!(outcome.collisions.is_empty());
    };
}

#[test]
fn writes_through_symlinks_are_prevented_even_if_overwriting_is_allowed() {
    let mut opts = opts_from_probe();
    // with overwrite mode
    opts.overwrite_existing = true;
    let (source_tree, destination, _index, outcome) =
        checkout_index_in_tmp_dir(opts.clone(), "make_dangerous_symlink").unwrap();

    let source_files = dir_structure(&source_tree);
    let worktree_files = dir_structure(&destination);

    if opts.fs.ignore_case {
        assert_eq!(
            stripped_prefix(&source_tree, &source_files),
            paths(["A-dir/a", "A-file", "fake-dir/b", "fake-file"]),
        );
        assert_eq!(
            stripped_prefix(&destination, &worktree_files),
            paths(["A-dir/a", "A-file", "FAKE-DIR", "FAKE-FILE"]),
        );
        assert!(outcome.collisions.is_empty());
    } else {
        let expected = ["A-dir/a", "A-file", "FAKE-DIR", "FAKE-FILE", "fake-dir/b", "fake-file"];
        assert_eq!(stripped_prefix(&source_tree, &source_files), paths(expected));
        assert_eq!(stripped_prefix(&destination, &worktree_files), paths(expected));
        assert!(outcome.collisions.is_empty());
    };
}

#[test]
fn overwriting_files_and_lone_directories_works() {
    let mut opts = opts_from_probe();
    opts.overwrite_existing = true;
    opts.destination_is_initially_empty = false;
    let (_source_tree, destination, _index, outcome) = checkout_index_in_tmp_dir_opts(
        opts.clone(),
        "make_mixed_without_submodules",
        |_| true,
        |d| {
            let empty = d.join("empty");
            symlink::symlink_dir(d.join(".."), &empty)?; // empty is symlink to the directory above
            std::fs::write(d.join("executable"), b"foo")?; // executable is regular file and has different content
            let dir = d.join("dir");
            std::fs::create_dir(&dir)?;
            std::fs::create_dir(dir.join("content"))?; // 'content' is a directory now

            let dir = dir.join("sub-dir");
            std::fs::create_dir(&dir)?;

            symlink::symlink_dir(empty, dir.join("symlink"))?; // 'symlink' is a symlink to another file
            Ok(())
        },
    )
    .unwrap();

    assert!(outcome.collisions.is_empty());

    assert_eq!(
        stripped_prefix(&destination, &dir_structure(&destination)),
        paths(["dir/content", "dir/sub-dir/symlink", "empty", "executable"])
    );
    let meta = std::fs::symlink_metadata(destination.path().join("empty")).unwrap();
    assert!(meta.is_file(), "'empty' is now a file");
    assert_eq!(meta.len(), 0, "'empty' is indeed empty");

    let exe = destination.path().join("executable");
    assert_eq!(
        std::fs::read(&exe).unwrap(),
        b"content",
        "'exe' has the correct content"
    );

    let meta = std::fs::symlink_metadata(exe).unwrap();
    assert!(meta.is_file());
    if opts.fs.executable_bit {
        #[cfg(unix)]
        assert_eq!(meta.mode() & 0o700, 0o700, "the executable bit is set where supported");
    }

    assert_eq!(
        std::fs::read(destination.path().join("dir/content")).unwrap(),
        b"other content"
    );

    let symlink = destination.path().join("dir/sub-dir/symlink");
    // on windows, git won't create symlinks as its probe won't detect the capability, even though we do.
    assert_eq!(std::fs::symlink_metadata(&symlink).unwrap().is_symlink(), cfg!(unix));
    assert_eq!(std::fs::read(symlink).unwrap(), b"other content");
}

#[test]
fn symlinks_become_files_if_disabled() -> crate::Result {
    let mut opts = opts_from_probe();
    opts.fs.symlink = false;
    let (source_tree, destination, _index, outcome) =
        checkout_index_in_tmp_dir(opts.clone(), "make_mixed_without_submodules")?;

    assert_equality(&source_tree, &destination, opts.fs.symlink)?;
    assert!(outcome.collisions.is_empty());
    Ok(())
}

#[test]
fn allow_or_disallow_symlinks() -> crate::Result {
    let mut opts = opts_from_probe();
    for allowed in &[false, true] {
        opts.fs.symlink = *allowed;
        let (source_tree, destination, _index, outcome) =
            checkout_index_in_tmp_dir(opts.clone(), "make_mixed_without_submodules")?;

        assert_equality(&source_tree, &destination, opts.fs.symlink)?;
        assert!(outcome.collisions.is_empty());
    }
    Ok(())
}

#[test]
fn keep_going_collects_results() {
    let mut opts = opts_from_probe();
    opts.keep_going = true;
    let count = AtomicUsize::default();
    let (_source_tree, destination, _index, outcome) = checkout_index_in_tmp_dir_opts(
        opts,
        "make_mixed_without_submodules",
        |_id| {
            if let Ok(_) = count.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                (current < 2).then(|| current + 1)
            }) {
                false
            } else {
                true
            }
        },
        |_| Ok(()),
    )
    .unwrap();

    if multi_threaded() {
        assert_eq!(
            outcome.errors.len(),
            2,
            "content changes due to non-deterministic nature of racy threads"
        )
    } else {
        assert_eq!(
            outcome
                .errors
                .iter()
                .map(|r| r.path.to_path_lossy().into_owned())
                .collect::<Vec<_>>(),
            paths(if cfg!(unix) {
                ["dir/content", "empty"]
            } else {
                // not actually a symlink anymore, even though symlinks are supported but git think differently.
                ["dir/content", "dir/sub-dir/symlink"]
            })
        );
    }

    if multi_threaded() {
        assert_eq!(dir_structure(&destination).len(), 2);
    } else {
        assert_eq!(
            stripped_prefix(&destination, &dir_structure(&destination)),
            paths(if cfg!(unix) {
                ["dir/sub-dir/symlink", "executable"]
            } else {
                ["empty", "executable"]
            }),
            "some files could not be created"
        );
    }

    assert!(outcome.collisions.is_empty());
}

#[test]
fn no_case_related_collisions_on_case_sensitive_filesystem() {
    let opts = opts_from_probe();
    if opts.fs.ignore_case {
        eprintln!("Skipping case-sensitive testing on what would be a case-insensitive file system");
        return;
    }
    let (source_tree, destination, index, outcome) =
        checkout_index_in_tmp_dir(opts.clone(), "make_ignorecase_collisions").unwrap();

    assert!(outcome.collisions.is_empty());
    let num_files = assert_equality(&source_tree, &destination, opts.fs.symlink).unwrap();
    assert_eq!(num_files, index.entries().len(), "it checks out all files");
}

#[test]
fn collisions_are_detected_on_a_case_insensitive_filesystem() {
    let opts = opts_from_probe();
    if !opts.fs.ignore_case {
        eprintln!("Skipping case-insensitive testing on what would be a case-sensitive file system");
        return;
    }
    let (source_tree, destination, _index, outcome) =
        checkout_index_in_tmp_dir(opts, "make_ignorecase_collisions").unwrap();

    let source_files = dir_structure(&source_tree);
    assert_eq!(
        stripped_prefix(&source_tree, &source_files),
        paths(["d", "file_x", "link-to-X", "x"]),
        "plenty of collisions prevent a checkout"
    );

    let dest_files = dir_structure(&destination);
    if multi_threaded() {
        assert_eq!(
            dest_files.len(),
            5,
            "can only assert on number as it's racily creating files so unclear which one clashes"
        );
    } else {
        assert_eq!(
            stripped_prefix(&destination, &dest_files),
            paths(["D/B", "D/C", "FILE_X", "X", "link-to-X"]),
            "we checkout files in order and generally handle collision detection differently, hence the difference"
        );
    }

    let error_kind = ErrorKind::AlreadyExists;
    #[cfg(windows)]
    let error_kind_dir = ErrorKind::PermissionDenied;
    #[cfg(not(windows))]
    let error_kind_dir = error_kind;

    if multi_threaded() {
        assert_eq!(
            outcome.collisions.len(),
            5,
            "can only assert on number as it's racily creating files so unclear which one clashes"
        );
    } else {
        assert_eq!(
            outcome.collisions,
            vec![
                Collision {
                    path: "FILE_x".into(),
                    error_kind,
                },
                Collision {
                    path: "d".into(),
                    error_kind: error_kind_dir,
                },
                Collision {
                    path: "file_X".into(),
                    error_kind,
                },
                Collision {
                    path: "file_x".into(),
                    error_kind,
                },
                Collision {
                    path: "x".into(),
                    error_kind,
                },
            ],
            "these files couldn't be checked out"
        );
    }
}

fn multi_threaded() -> bool {
    gix_features::parallel::num_threads(None) > 1
}

fn assert_equality(source_tree: &Path, destination: &TempDir, allow_symlinks: bool) -> crate::Result<usize> {
    let source_files = dir_structure(source_tree);
    let worktree_files = dir_structure(&destination);

    assert_eq!(
        stripped_prefix(source_tree, &source_files),
        stripped_prefix(&destination, &worktree_files),
    );

    let mut count = 0;
    for (source_file, worktree_file) in source_files.iter().zip(worktree_files.iter()) {
        count += 1;
        if !allow_symlinks && source_file.is_symlink() {
            assert!(!worktree_file.is_symlink());
            assert_eq!(fs::read(worktree_file)?.to_path()?, fs::read_link(source_file)?);
        } else {
            assert_eq!(fs::read(source_file)?, fs::read(worktree_file)?);
            #[cfg(unix)]
            assert_eq!(
                fs::symlink_metadata(source_file)?.mode() & 0o700,
                fs::symlink_metadata(worktree_file)?.mode() & 0o700,
                "permissions of source and checked out file are comparable"
            );
        }
    }
    Ok(count)
}

pub fn dir_structure<P: AsRef<std::path::Path>>(path: P) -> Vec<std::path::PathBuf> {
    let path = path.as_ref();
    let mut files: Vec<_> = walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.path() == path || !e.file_name().to_string_lossy().starts_with('.'))
        .flatten()
        .filter_map(|e| (!e.path().symlink_metadata().map_or(true, |m| m.is_dir())).then(|| e.path().to_path_buf()))
        .collect();
    files.sort();
    files
}

fn checkout_index_in_tmp_dir(
    opts: gix_worktree::checkout::Options,
    name: &str,
) -> crate::Result<(PathBuf, TempDir, gix_index::File, gix_worktree::checkout::Outcome)> {
    checkout_index_in_tmp_dir_opts(opts, name, |_d| true, |_| Ok(()))
}

fn checkout_index_in_tmp_dir_opts(
    opts: gix_worktree::checkout::Options,
    name: &str,
    mut allow_return_object: impl FnMut(&gix_hash::oid) -> bool + Send + Clone,
    prep_dest: impl Fn(&Path) -> std::io::Result<()>,
) -> crate::Result<(PathBuf, TempDir, gix_index::File, gix_worktree::checkout::Outcome)> {
    let source_tree = fixture_path(name);
    let git_dir = source_tree.join(".git");
    let mut index = gix_index::File::at(git_dir.join("index"), gix_hash::Kind::Sha1, Default::default())?;
    let odb = gix_odb::at(git_dir.join("objects"))?.into_inner().into_arc()?;
    let destination = tempfile::tempdir_in(std::env::current_dir()?)?;
    prep_dest(destination.path())?;

    let outcome = gix_worktree::checkout(
        &mut index,
        destination.path(),
        move |oid, buf| {
            if allow_return_object(oid) {
                odb.find_blob(oid, buf)
            } else {
                Err(gix_odb::find::existing_object::Error::NotFound { oid: oid.to_owned() })
            }
        },
        &mut progress::Discard,
        &mut progress::Discard,
        &AtomicBool::default(),
        opts,
    )?;
    Ok((source_tree, destination, index, outcome))
}

fn stripped_prefix(prefix: impl AsRef<Path>, source_files: &[PathBuf]) -> Vec<&Path> {
    source_files.iter().flat_map(|p| p.strip_prefix(&prefix)).collect()
}

fn probe_gitoxide_dir() -> crate::Result<gix_fs::Capabilities> {
    Ok(gix_fs::Capabilities::probe(
        std::env::current_dir()?.join("..").join(".git"),
    ))
}

fn opts_from_probe() -> gix_worktree::checkout::Options {
    gix_worktree::checkout::Options {
        fs: probe_gitoxide_dir().unwrap(),
        destination_is_initially_empty: true,
        thread_limit: gix_features::parallel::num_threads(None).into(),
        ..Default::default()
    }
}

fn paths<'a>(p: impl IntoIterator<Item = &'a str>) -> Vec<PathBuf> {
    p.into_iter().map(PathBuf::from).collect()
}
