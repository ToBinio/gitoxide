use crate::{data, find};

/// Describe how object can be located in an object store with built-in facilities to supports packs specifically.
///
/// ## Notes
///
/// Find effectively needs [generic associated types][issue] to allow a trait for the returned object type.
/// Until then, we will have to make due with explicit types and give them the potentially added features we want.
///
/// Furthermore, despite this trait being in `gix-pack`, it leaks knowledge about objects potentially not being packed.
/// This is a necessary trade-off to allow this trait to live in `gix-pack` where it is used in functions to create a pack.
///
/// [issue]: https://github.com/rust-lang/rust/issues/44265
pub trait Find {
    /// The error returned by [`try_find()`][Find::try_find()]
    type Error: std::error::Error + Send + Sync + 'static;

    /// Returns true if the object exists in the database.
    fn contains(&self, id: impl AsRef<gix_hash::oid>) -> bool;

    /// Find an object matching `id` in the database while placing its raw, decoded data into `buffer`.
    /// A `pack_cache` can be used to speed up subsequent lookups, set it to [`crate::cache::Never`] if the
    /// workload isn't suitable for caching.
    ///
    /// Returns `Some((<object data>, <pack location if packed>))` if it was present in the database,
    /// or the error that occurred during lookup or object retrieval.
    fn try_find<'a>(
        &self,
        id: impl AsRef<gix_hash::oid>,
        buffer: &'a mut Vec<u8>,
    ) -> Result<Option<(gix_object::Data<'a>, Option<data::entry::Location>)>, Self::Error> {
        self.try_find_cached(id, buffer, &mut crate::cache::Never)
    }

    /// Like [`Find::try_find()`], but with support for controlling the pack cache.
    /// A `pack_cache` can be used to speed up subsequent lookups, set it to [`crate::cache::Never`] if the
    /// workload isn't suitable for caching.
    ///
    /// Returns `Some((<object data>, <pack location if packed>))` if it was present in the database,
    /// or the error that occurred during lookup or object retrieval.
    fn try_find_cached<'a>(
        &self,
        id: impl AsRef<gix_hash::oid>,
        buffer: &'a mut Vec<u8>,
        pack_cache: &mut impl crate::cache::DecodeEntry,
    ) -> Result<Option<(gix_object::Data<'a>, Option<data::entry::Location>)>, Self::Error>;

    /// Find the packs location where an object with `id` can be found in the database, or `None` if there is no pack
    /// holding the object.
    ///
    /// _Note_ that this is always None if the object isn't packed even though it exists as loose object.
    fn location_by_oid(&self, id: impl AsRef<gix_hash::oid>, buf: &mut Vec<u8>) -> Option<data::entry::Location>;

    /// Obtain a vector of all offsets, in index order, along with their object id.
    fn pack_offsets_and_oid(&self, pack_id: u32) -> Option<Vec<(data::Offset, gix_hash::ObjectId)>>;

    /// Return the [`find::Entry`] for `location` if it is backed by a pack.
    ///
    /// Note that this is only in the interest of avoiding duplicate work during pack generation.
    /// Pack locations can be obtained from [`Find::try_find()`].
    ///
    /// # Notes
    ///
    /// Custom implementations might be interested in providing their own meta-data with `object`,
    /// which currently isn't possible as the `Locate` trait requires GATs to work like that.
    fn entry_by_location(&self, location: &data::entry::Location) -> Option<find::Entry>;
}

mod ext {
    use gix_object::{BlobRef, CommitRef, CommitRefIter, Kind, ObjectRef, TagRef, TagRefIter, TreeRef, TreeRefIter};

    use crate::find;

    macro_rules! make_obj_lookup {
        ($method:ident, $object_variant:path, $object_kind:path, $object_type:ty) => {
            /// Like [`find(…)`][Self::find()], but flattens the `Result<Option<_>>` into a single `Result` making a non-existing object an error
            /// while returning the desired object type.
            fn $method<'a>(
                &self,
                id: impl AsRef<gix_hash::oid>,
                buffer: &'a mut Vec<u8>,
            ) -> Result<($object_type, Option<crate::data::entry::Location>), find::existing_object::Error<Self::Error>>
            {
                let id = id.as_ref();
                self.try_find(id, buffer)
                    .map_err(find::existing_object::Error::Find)?
                    .ok_or_else(|| find::existing_object::Error::NotFound {
                        oid: id.as_ref().to_owned(),
                    })
                    .and_then(|(o, l)| {
                        o.decode()
                            .map_err(find::existing_object::Error::Decode)
                            .map(|o| (o, l))
                    })
                    .and_then(|(o, l)| match o {
                        $object_variant(o) => return Ok((o, l)),
                        _other => Err(find::existing_object::Error::ObjectKind {
                            expected: $object_kind,
                        }),
                    })
            }
        };
    }

    macro_rules! make_iter_lookup {
        ($method:ident, $object_kind:path, $object_type:ty, $into_iter:tt) => {
            /// Like [`find(…)`][Self::find()], but flattens the `Result<Option<_>>` into a single `Result` making a non-existing object an error
            /// while returning the desired iterator type.
            fn $method<'a>(
                &self,
                id: impl AsRef<gix_hash::oid>,
                buffer: &'a mut Vec<u8>,
            ) -> Result<($object_type, Option<crate::data::entry::Location>), find::existing_iter::Error<Self::Error>> {
                let id = id.as_ref();
                self.try_find(id, buffer)
                    .map_err(find::existing_iter::Error::Find)?
                    .ok_or_else(|| find::existing_iter::Error::NotFound {
                        oid: id.as_ref().to_owned(),
                    })
                    .and_then(|(o, l)| {
                        o.$into_iter()
                            .ok_or_else(|| find::existing_iter::Error::ObjectKind {
                                expected: $object_kind,
                            })
                            .map(|i| (i, l))
                    })
            }
        };
    }

    /// An extension trait with convenience functions.
    pub trait FindExt: super::Find {
        /// Like [`try_find(…)`][super::Find::try_find()], but flattens the `Result<Option<_>>` into a single `Result` making a non-existing object an error.
        fn find<'a>(
            &self,
            id: impl AsRef<gix_hash::oid>,
            buffer: &'a mut Vec<u8>,
        ) -> Result<(gix_object::Data<'a>, Option<crate::data::entry::Location>), find::existing::Error<Self::Error>>
        {
            let id = id.as_ref();
            self.try_find(id, buffer)
                .map_err(find::existing::Error::Find)?
                .ok_or_else(|| find::existing::Error::NotFound {
                    oid: id.as_ref().to_owned(),
                })
        }

        make_obj_lookup!(find_commit, ObjectRef::Commit, Kind::Commit, CommitRef<'a>);
        make_obj_lookup!(find_tree, ObjectRef::Tree, Kind::Tree, TreeRef<'a>);
        make_obj_lookup!(find_tag, ObjectRef::Tag, Kind::Tag, TagRef<'a>);
        make_obj_lookup!(find_blob, ObjectRef::Blob, Kind::Blob, BlobRef<'a>);
        make_iter_lookup!(find_commit_iter, Kind::Blob, CommitRefIter<'a>, try_into_commit_iter);
        make_iter_lookup!(find_tree_iter, Kind::Tree, TreeRefIter<'a>, try_into_tree_iter);
        make_iter_lookup!(find_tag_iter, Kind::Tag, TagRefIter<'a>, try_into_tag_iter);
    }

    impl<T: super::Find> FindExt for T {}
}
pub use ext::FindExt;

mod find_impls {
    use std::{ops::Deref, rc::Rc};

    use gix_hash::oid;

    use crate::{data, find};

    impl<T> crate::Find for &T
    where
        T: crate::Find,
    {
        type Error = T::Error;

        fn contains(&self, id: impl AsRef<oid>) -> bool {
            (*self).contains(id)
        }

        fn try_find_cached<'a>(
            &self,
            id: impl AsRef<oid>,
            buffer: &'a mut Vec<u8>,
            pack_cache: &mut impl crate::cache::DecodeEntry,
        ) -> Result<Option<(gix_object::Data<'a>, Option<data::entry::Location>)>, Self::Error> {
            (*self).try_find_cached(id, buffer, pack_cache)
        }

        fn location_by_oid(&self, id: impl AsRef<oid>, buf: &mut Vec<u8>) -> Option<data::entry::Location> {
            (*self).location_by_oid(id, buf)
        }

        fn pack_offsets_and_oid(&self, pack_id: u32) -> Option<Vec<(data::Offset, gix_hash::ObjectId)>> {
            (*self).pack_offsets_and_oid(pack_id)
        }

        fn entry_by_location(&self, location: &data::entry::Location) -> Option<find::Entry> {
            (*self).entry_by_location(location)
        }
    }

    impl<T> super::Find for std::sync::Arc<T>
    where
        T: super::Find,
    {
        type Error = T::Error;

        fn contains(&self, id: impl AsRef<oid>) -> bool {
            self.deref().contains(id)
        }

        fn try_find_cached<'a>(
            &self,
            id: impl AsRef<oid>,
            buffer: &'a mut Vec<u8>,
            pack_cache: &mut impl crate::cache::DecodeEntry,
        ) -> Result<Option<(gix_object::Data<'a>, Option<data::entry::Location>)>, Self::Error> {
            self.deref().try_find_cached(id, buffer, pack_cache)
        }

        fn location_by_oid(&self, id: impl AsRef<oid>, buf: &mut Vec<u8>) -> Option<data::entry::Location> {
            self.deref().location_by_oid(id, buf)
        }

        fn pack_offsets_and_oid(&self, pack_id: u32) -> Option<Vec<(data::Offset, gix_hash::ObjectId)>> {
            self.deref().pack_offsets_and_oid(pack_id)
        }

        fn entry_by_location(&self, object: &data::entry::Location) -> Option<find::Entry> {
            self.deref().entry_by_location(object)
        }
    }

    impl<T> super::Find for Rc<T>
    where
        T: super::Find,
    {
        type Error = T::Error;

        fn contains(&self, id: impl AsRef<oid>) -> bool {
            self.deref().contains(id)
        }

        fn try_find_cached<'a>(
            &self,
            id: impl AsRef<oid>,
            buffer: &'a mut Vec<u8>,
            pack_cache: &mut impl crate::cache::DecodeEntry,
        ) -> Result<Option<(gix_object::Data<'a>, Option<data::entry::Location>)>, Self::Error> {
            self.deref().try_find_cached(id, buffer, pack_cache)
        }

        fn location_by_oid(&self, id: impl AsRef<oid>, buf: &mut Vec<u8>) -> Option<data::entry::Location> {
            self.deref().location_by_oid(id, buf)
        }

        fn pack_offsets_and_oid(&self, pack_id: u32) -> Option<Vec<(data::Offset, gix_hash::ObjectId)>> {
            self.deref().pack_offsets_and_oid(pack_id)
        }

        fn entry_by_location(&self, location: &data::entry::Location) -> Option<find::Entry> {
            self.deref().entry_by_location(location)
        }
    }

    impl<T> super::Find for Box<T>
    where
        T: super::Find,
    {
        type Error = T::Error;

        fn contains(&self, id: impl AsRef<oid>) -> bool {
            self.deref().contains(id)
        }

        fn try_find_cached<'a>(
            &self,
            id: impl AsRef<oid>,
            buffer: &'a mut Vec<u8>,
            pack_cache: &mut impl crate::cache::DecodeEntry,
        ) -> Result<Option<(gix_object::Data<'a>, Option<data::entry::Location>)>, Self::Error> {
            self.deref().try_find_cached(id, buffer, pack_cache)
        }

        fn location_by_oid(&self, id: impl AsRef<oid>, buf: &mut Vec<u8>) -> Option<data::entry::Location> {
            self.deref().location_by_oid(id, buf)
        }

        fn pack_offsets_and_oid(&self, pack_id: u32) -> Option<Vec<(data::Offset, gix_hash::ObjectId)>> {
            self.deref().pack_offsets_and_oid(pack_id)
        }

        fn entry_by_location(&self, location: &data::entry::Location) -> Option<find::Entry> {
            self.deref().entry_by_location(location)
        }
    }
}
