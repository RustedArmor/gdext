/*
 * Copyright (c) godot-rust; Bromeon and contributors.
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

// Result<..., ()> is used. But we don't have more error info. https://rust-lang.github.io/rust-clippy/master/index.html#result_unit_err.
// We may want to change () to something like godot::meta::IoError, or a domain-specific one, in the future.
#![allow(clippy::result_unit_err)]

use godot_ffi as sys;

use crate::builtin::collections::extend_buffer::ExtendBuffer;
use crate::builtin::*;
use crate::meta::{AsArg, ToGodot};
use std::mem::size_of;
use std::{fmt, ops, ptr};
use sys::types::*;
use sys::{ffi_methods, interface_fn, ExtVariantType, GodotFfi};

use crate::classes::file_access::CompressionMode;
use crate::meta;
use crate::obj::EngineEnum;

// FIXME remove dependency on these types.
use sys::{__GdextString, __GdextType};
// TODO(bromeon): ensure and test that all element types can be packed.
// Many builtin types don't have a #[repr] themselves, but they are used in packed arrays, which assumes certain size and alignment.
// This is mostly a problem for as_slice(), which reinterprets the FFI representation into the "frontend" type like GString.

/// Defines and implements a single packed array type. This macro is not hygienic and is meant to
/// be used only in the current module.
macro_rules! impl_packed_array {
    (
        // Name of the type to define, e.g. `PackedByteArray`.
        type_name: $PackedArray:ident,
        // Name of the VariantType constant, e.g. `PACKED_BYTE_ARRAY`.
        variant_type: $VariantType:ident,
        // Type of elements contained in the array, e.g. `u8`.
        element_type: $Element:ty,
        // Name of wrapped opaque type, e.g. `OpaquePackedByteArray`.
        opaque_type: $Opaque:ty,
        // Name of inner type, e.g. `InnerPackedByteArray`.
        inner_type: $Inner:ident,
        // Name of type that represents elements in function call arguments, e.g. `i64`. See `to_arg()`.
        argument_type: $Arg:ty,
        // Type that is returned from `$operator_index` and `$operator_index_const`.
        return_type: $IndexRetType:ty,
        // Name of constructor function from `Array` from FFI, e.g. `packed_byte_array_from_array`.
        from_array: $from_array:ident,
        // Name of index operator from FFI, e.g. `packed_byte_array_operator_index`.
        operator_index: $operator_index:ident,
        // Name of const index operator from FFI, e.g. `packed_byte_array_operator_index_const`.
        operator_index_const: $operator_index_const:ident,
        // Invocation passed to `impl_builtin_traits!` macro.
        trait_impls: {
            $($trait_impls:tt)*
        },
    ) => {
        #[doc = concat!("Space-efficient array of [`", stringify!($Element), "`] elements.")]
        ///
        /// Check out the [book](https://godot-rust.github.io/book/godot-api/builtins.html#packed-arrays) for a tutorial on packed arrays.
        ///
        /// Note that, unlike `Array`, this type has value semantics: each copy will be independent
        /// of the original. Under the hood, Godot uses copy-on-write, so copies are still cheap
        /// to make.
        ///
        /// # Registering properties
        ///
        /// You can use both `#[var]` and `#[export]` with packed arrays. However, since they use copy-on-write, GDScript (for `#[var]`) and the
        /// editor (for `#[export]`) will effectively keep an independent copy of the array. Writes to the packed array from Rust are thus not
        /// reflected on the other side -- you may need to replace the entire array.
        ///
        /// See also [godot/#76150](https://github.com/godotengine/godot/issues/76150) for details.
        ///
        /// # Thread safety
        ///
        #[doc = concat!("Usage is safe if the `", stringify!($PackedArray), "`")]
        /// is used on a single thread only. Concurrent reads on different threads are also safe,
        /// but any writes must be externally synchronized. The Rust compiler will enforce this as
        /// long as you use only Rust threads, but it cannot protect against concurrent modification
        /// on other threads (e.g. created through GDScript).
        pub struct $PackedArray {
            opaque: $Opaque,
        }

        impl $PackedArray {
            fn from_opaque(opaque: $Opaque) -> Self {
                Self { opaque }
            }

            /// Constructs an empty array.
            pub fn new() -> Self {
                Self::default()
            }

            /// Returns a copy of the value at the specified index, or `None` if out-of-bounds.
            ///
            /// If you know the index is valid, use the `[]` operator (`Index`/`IndexMut` traits) instead.
            pub fn get(&self, index: usize) -> Option<$Element> {
                let ptr = self.ptr_or_none(index)?;

                // SAFETY: if index was out of bounds, `ptr` would be `None` and return early.
                unsafe { Some((*ptr).clone()) }
            }

            /// Returns `true` if the array contains the given value.
            ///
            /// _Godot equivalent: `has`_
            #[doc(alias = "has")]
            pub fn contains(&self, value: impl AsArg<$Element>) -> bool {
                self.as_inner().has(value.into_arg().into_packed_arg())
            }

            /// Returns the number of times a value is in the array.
            pub fn count(&self, value: impl AsArg<$Element>) -> usize {
                to_usize(self.as_inner().count(value.into_arg().into_packed_arg()))
            }

            /// Returns the number of elements in the array. Equivalent of `size()` in Godot.
            #[doc(alias = "size")]
            pub fn len(&self) -> usize {
                to_usize(self.as_inner().size())
            }

            /// Returns `true` if the array is empty.
            pub fn is_empty(&self) -> bool {
                self.as_inner().is_empty()
            }

            /// Clears the array, removing all elements.
            pub fn clear(&mut self) {
                self.as_inner().clear();
            }

            /// Appends an element to the end of the array. Equivalent of `append` and `push_back`
            /// in GDScript.
            #[doc(alias = "append")]
            #[doc(alias = "push_back")]
            pub fn push(&mut self, value: impl AsArg<$Element>) {
                self.as_inner().push_back(value.into_arg().into_packed_arg());
            }

            /// Inserts a new element at a given index in the array. The index must be valid, or at
            /// the end of the array (`index == len()`).
            ///
            /// Note: On large arrays, this method is much slower than `push` as it will move all
            /// the array's elements after the inserted element. The larger the array, the slower
            /// `insert` will be.
            pub fn insert(&mut self, index: usize, value: impl AsArg<$Element>) {
                // Intentional > and not >=.
                if index > self.len() {
                    self.panic_out_of_bounds(index);
                }

                self.as_inner().insert(to_i64(index), value.into_arg().into_packed_arg());
            }

            /// Removes and returns the element at the specified index. Similar to `remove_at` in
            /// GDScript, but also returns the removed value.
            ///
            /// On large arrays, this method is much slower than `pop_back` as it will move all the array's
            /// elements after the removed element. The larger the array, the slower `remove` will be.
            ///
            /// # Panics
            ///
            /// If `index` is out of bounds.
            // Design note: This returns the removed value instead of `()` for consistency with
            // `Array` and with `Vec::remove`. Compared to shifting all the subsequent array
            // elements to their new position, the overhead of retrieving this element is trivial.
            #[doc(alias = "remove_at")]
            pub fn remove(&mut self, index: usize) -> $Element {
                let element = self[index].clone(); // panics on out-of-bounds
                self.as_inner().remove_at(to_i64(index));
                element
            }

            /// Assigns the given value to all elements in the array. This can be used together
            /// with `resize` to create an array with a given size and initialized elements.
            pub fn fill(&mut self, value: impl AsArg<$Element>) {
                self.as_inner().fill(value.into_arg().into_packed_arg());
            }

            /// Resizes the array to contain a different number of elements. If the new size is
            /// smaller, elements are removed from the end. If the new size is larger, new elements
            /// are set to [`Default::default()`].
            pub fn resize(&mut self, size: usize) {
                self.as_inner().resize(to_i64(size));
            }

            /// Appends another array at the end of this array. Equivalent of `append_array` in GDScript.
            pub fn extend_array(&mut self, other: &$PackedArray) {
                self.as_inner().append_array(other);
            }

            /// Converts this array to a Rust vector, making a copy of its contents.
            pub fn to_vec(&self) -> Vec<$Element> {
                let len = self.len();
                let mut vec = Vec::with_capacity(len);
                if len > 0 {
                    let ptr = self.ptr(0);
                    for offset in 0..to_isize(len) {
                        // SAFETY: Packed arrays are stored contiguously in memory, so we can use
                        // pointer arithmetic instead of going through `$operator_index_const` for
                        // every index.
                        // Note that we do need to use `.clone()` because `GString` is refcounted;
                        // we can't just do a memcpy.
                        let element = unsafe { (*ptr.offset(offset)).clone() };
                        vec.push(element);
                    }
                }
                vec
            }

            /// Returns a sub-range `begin..end`, as a new packed array.
            ///
            /// This method is called `slice()` in Godot.
            /// The values of `begin` (inclusive) and `end` (exclusive) will be clamped to the array size.
            ///
            /// To obtain Rust slices, see [`as_slice`][Self::as_slice] and [`as_mut_slice`][Self::as_mut_slice].
            #[doc(alias = "slice")]
            // TODO(v0.3): change to i32 like NodePath::slice/subpath() and support+test negative indices.
            pub fn subarray(&self, begin: usize, end: usize) -> Self {
                let len = self.len();
                let begin = begin.min(len);
                let end = end.min(len);
                self.as_inner().slice(to_i64(begin), to_i64(end))
            }

            /// Returns a shared Rust slice of the array.
            ///
            /// The resulting slice can be further subdivided or converted into raw pointers.
            ///
            /// See also [`as_mut_slice`][Self::as_mut_slice] to get exclusive slices, and
            /// [`subarray`][Self::subarray] to get a sub-array as a copy.
            pub fn as_slice(&self) -> &[$Element] {
                if self.is_empty() {
                    &[]
                } else {
                    let data = self.ptr(0);

                    // SAFETY: PackedArray holds `len` elements in contiguous storage, all of which are initialized.
                    // The array uses copy-on-write semantics, so the slice may be aliased, but copies will use a new allocation.
                    unsafe {
                        std::slice::from_raw_parts(data, self.len())
                    }
                }
            }

            /// Returns an exclusive Rust slice of the array.
            ///
            /// The resulting slice can be further subdivided or converted into raw pointers.
            ///
            /// See also [`as_slice`][Self::as_slice] to get shared slices, and
            /// [`subarray`][Self::subarray] to get a sub-array as a copy.
            pub fn as_mut_slice(&mut self) -> &mut [$Element] {
                if self.is_empty() {
                    &mut []
                } else {
                    let data = self.ptr_mut(0);

                    // SAFETY: PackedArray holds `len` elements in contiguous storage, all of which are initialized.
                    // The array uses copy-on-write semantics. ptr_mut() triggers a copy if non-unique, after which the slice is never aliased.
                    unsafe {
                        std::slice::from_raw_parts_mut(data, self.len())
                    }
                }
            }

            /// Searches the array for the first occurrence of a value and returns its index, or
            /// `None` if not found. Starts searching at index `from`; pass `None` to search the
            /// entire array.
            pub fn find(&self, value: impl AsArg<$Element>, from: Option<usize>) -> Option<usize> {
                let from = to_i64(from.unwrap_or(0));
                let index = self.as_inner().find(value.into_arg().into_packed_arg(), from);
                if index >= 0 {
                    Some(index.try_into().unwrap())
                } else {
                    None
                }
            }

            /// Searches the array backwards for the last occurrence of a value and returns its
            /// index, or `None` if not found. Starts searching at index `from`; pass `None` to
            /// search the entire array.
            pub fn rfind(&self, value: impl AsArg<$Element>, from: Option<usize>) -> Option<usize> {
                let from = from.map(to_i64).unwrap_or(-1);
                let index = self.as_inner().rfind(value.into_arg().into_packed_arg(), from);
                // It's not documented, but `rfind` returns -1 if not found.
                if index >= 0 {
                    Some(to_usize(index))
                } else {
                    None
                }
            }

            /// Finds the index of an existing value in a _sorted_ array using binary search.
            ///
            /// If the value is not present in the array, returns the insertion index that would maintain sorting order.
            ///
            /// Calling `bsearch()` on an unsorted array results in unspecified (but safe) behavior.
            pub fn bsearch(&self, value: impl AsArg<$Element>) -> usize {
                to_usize(self.as_inner().bsearch(value.into_arg().into_packed_arg(), true))
            }

            /// Reverses the order of the elements in the array.
            pub fn reverse(&mut self) {
                self.as_inner().reverse();
            }

            /// Sorts the elements of the array in ascending order.
            ///
            /// This sort is [stable](https://en.wikipedia.org/wiki/Sorting_algorithm#Stability), since elements inside packed arrays are
            /// indistinguishable. Relative order between equal elements thus isn't observable.
            pub fn sort(&mut self) {
                self.as_inner().sort();
            }

            // Include specific functions in the code only if the Packed*Array provides the function.
            declare_packed_array_conversion_fns!($PackedArray);

            /// # Panics
            ///
            /// Always.
            fn panic_out_of_bounds(&self, index: usize) -> ! {
                panic!("Array index {index} is out of bounds: length is {}", self.len());
            }

            /// Returns a pointer to the element at the given index.
            ///
            /// # Panics
            ///
            /// If `index` is out of bounds.
            fn ptr(&self, index: usize) -> *const $Element {
                self.ptr_or_none(index).unwrap_or_else(|| self.panic_out_of_bounds(index))
            }

            /// Returns a pointer to the element at the given index, or `None` if out of bounds.
            fn ptr_or_none(&self, index: usize) -> Option<*const $Element> {
                // SAFETY: The packed array index operators return a null pointer on out-of-bounds.
                let item_ptr: *const $IndexRetType = unsafe {
                    interface_fn!($operator_index_const)(self.sys(), to_i64(index))
                };

                if item_ptr.is_null() {
                    None
                } else {
                    Some(item_ptr as *const $Element)
                }
            }

            /// Returns a mutable pointer to the element at the given index.
            ///
            /// # Panics
            ///
            /// If `index` is out of bounds.
            fn ptr_mut(&mut self, index: usize) -> *mut $Element {
                // SAFETY: The packed array index operators return a null pointer on out-of-bounds.
                let item_ptr: *mut $IndexRetType = unsafe {
                    interface_fn!($operator_index)(self.sys_mut(), to_i64(index))
                };

                if item_ptr.is_null() {
                    self.panic_out_of_bounds(index)
                } else {
                    item_ptr as *mut $Element
                }
            }

            #[doc(hidden)]
            pub fn as_inner(&self) -> inner::$Inner<'_> {
                inner::$Inner::from_outer(self)
            }

            /// Create array filled with default elements.
            fn default_with_size(n: usize) -> Self {
                let mut array = Self::new();
                array.resize(n);
                array
            }

            /// Drops all elements in `self` starting from `dst` and replaces them with data from an array of values.
            /// `dst` must be a valid index, even if `len` is zero.
            ///
            /// # Safety
            ///
            /// * `src` must be valid slice of data with `len` size.
            /// * `src` must not point to `self` data.
            /// * `len` must be equal to `self.len() - dst`.
            /// * Source data must not be dropped later.
            unsafe fn move_from_slice(&mut self, src: *const $Element, dst: usize, len: usize) {
                let ptr = self.ptr_mut(dst);
                debug_assert_eq!(len, self.len() - dst, "length precondition violated");
                // Drops all elements in place. Drop impl must not panic.
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(ptr, len));
                // Copy is okay since all elements are dropped.
                ptr.copy_from_nonoverlapping(src, len);
            }
        }

        impl_builtin_traits! {
            for $PackedArray {
                $($trait_impls)*
            }
        }

        impl ops::Index<usize> for $PackedArray {
            type Output = $Element;

            fn index(&self, index: usize) -> &Self::Output {
                let ptr = self.ptr(index);
                // SAFETY: `ptr` checked bounds.
                unsafe { &*ptr }
            }
        }

        impl ops::IndexMut<usize> for $PackedArray {
            fn index_mut(&mut self, index: usize) -> &mut Self::Output {
                let ptr = self.ptr_mut(index);
                // SAFETY: `ptr` checked bounds.
                unsafe { &mut *ptr }
            }
        }

        #[doc = concat!("Creates a `", stringify!($PackedArray), "` from the given Rust array.")]
        impl<const N: usize> From<&[$Element; N]> for $PackedArray {
            fn from(arr: &[$Element; N]) -> Self {
                Self::from(&arr[..])
            }
        }

        #[doc = concat!("Creates a `", stringify!($PackedArray), "` from the given slice.")]
        impl From<&[$Element]> for $PackedArray {
            fn from(slice: &[$Element]) -> Self {
                if slice.is_empty() {
                    return Self::new();
                }
                let mut array = Self::default_with_size(slice.len());

                // SAFETY: The array contains exactly `len` elements, stored contiguously in memory.
                let dst = unsafe { std::slice::from_raw_parts_mut(array.ptr_mut(0), slice.len()) };
                dst.clone_from_slice(slice);
                array
            }
        }

        #[doc = concat!("Creates a `", stringify!($PackedArray), "` from the given Rust array.")]
        impl<const N: usize> From<[$Element; N]> for $PackedArray {
            fn from(arr: [$Element; N]) -> Self {
                if N == 0 {
                    return Self::new();
                }
                let mut packed_array = Self::default_with_size(N);

                // Not using forget() so if move_from_slice somehow panics then there is no double-free.
                let arr = std::mem::ManuallyDrop::new(arr);

                // SAFETY: The packed array contains exactly N elements and the source array will be forgotten.
                unsafe {
                    packed_array.move_from_slice(arr.as_ptr(), 0, N);
                }
                packed_array
            }
        }

        #[doc = concat!("Creates a `", stringify!($PackedArray), "` from the given Rust vec.")]
        impl From<Vec<$Element>> for $PackedArray {
            fn from(mut vec: Vec<$Element>) -> Self {
                if vec.is_empty() {
                    return Self::new();
                }
                let len = vec.len();
                let mut array = Self::default_with_size(len);

                // SAFETY: The packed array and vector contain exactly `len` elements.
                // The vector is forcibly set to empty, so its contents are forgotten.
                unsafe {
                    vec.set_len(0);
                    array.move_from_slice(vec.as_ptr(), 0, len);
                }
                array
            }
        }

        #[doc = concat!("Creates a `", stringify!($PackedArray), "` from an iterator.")]
        ///
        /// # Performance note
        /// This uses the lower bound from `Iterator::size_hint()` to allocate memory up front. If the iterator returns
        /// more than that number of elements, it falls back to reading elements into a fixed-size buffer before adding
        /// them all efficiently as a batch.
        ///
        /// # Panics
        /// - If the iterator's `size_hint()` returns an incorrect lower bound (which is a breach of the `Iterator` protocol).
        impl FromIterator<$Element> for $PackedArray {
            fn from_iter<I: IntoIterator<Item = $Element>>(iter: I) -> Self {
                let mut array = $PackedArray::default();
                array.extend(iter);
                array
            }
        }

        #[doc = concat!("Extends a`", stringify!($PackedArray), "` with the contents of an iterator.")]
        ///
        /// # Performance note
        /// This uses the lower bound from `Iterator::size_hint()` to allocate memory up front. If the iterator returns
        /// more than that number of elements, it falls back to reading elements into a fixed-size buffer before adding
        /// them all efficiently as a batch.
        ///
        /// # Panics
        /// - If the iterator's `size_hint()` returns an incorrect lower bound (which is a breach of the `Iterator` protocol).
        impl Extend<$Element> for $PackedArray {
            fn extend<I: IntoIterator<Item = $Element>>(&mut self, iter: I) {
                // This function is complicated, but with good reason. The problem is that we don't know the length of
                // the `Iterator` ahead of time; all we get is its `size_hint()`.
                //
                // There are at least two categories of iterators that are common in the wild, for which we'd want good performance:
                //
                // 1. The length is known: `size_hint()` returns the exact size, e.g. just iterating over a `Vec` or `BTreeSet`.
                // 2. The length is unknown: `size_hint()` returns 0, e.g. `Filter`, `FlatMap`, `FromFn`.
                //
                // A number of implementations are possible, which were benchmarked for 1000 elements of type `i32`:
                //
                // - Simply call `push()` in a loop:
                //   6.1 µs whether or not the length is known.
                // - First `collect()` the `Iterator` into a `Vec`, call `self.resize()` to make room, then move out of the `Vec`:
                //   0.78 µs if the length is known, 1.62 µs if the length is unknown.
                //   It also requires additional temporary memory to hold all elements.
                // - The strategy implemented below:
                //   0.097 µs if the length is known, 0.49 µs if the length is unknown.
                //
                // The implementation of `Vec` in the standard library deals with this by repeatedly `reserve()`ing
                // whatever `size_hint()` returned, but we don't want to do that because the Godot API call to
                // `self.resize()` is relatively slow.

                let mut iter = iter.into_iter();
                // Cache the length to avoid repeated Godot API calls.
                let mut len = self.len();

                // Fast part.
                //
                // Use `Iterator::size_hint()` to pre-allocate the minimum number of elements in the iterator, then
                // write directly to the resulting slice. We can do this because `size_hint()` is required by the
                // `Iterator` contract to return correct bounds. Note that any bugs in it must not result in UB.
                let (size_hint_min, _size_hint_max) = iter.size_hint();
                if size_hint_min > 0 {
                    let capacity = len + size_hint_min;
                    self.resize(capacity);
                    for out_ref in &mut self.as_mut_slice()[len..] {
                        *out_ref = iter.next().expect("iterator returned fewer than size_hint().0 elements");
                    }
                    len = capacity;
                }

                // Slower part.
                //
                // While the iterator is still not finished, gather elements into a fixed-size buffer, then add them all
                // at once.
                //
                // Why not call `self.resize()` with fixed-size increments, like 32 elements at a time? Well, we might
                // end up over-allocating, and then need to trim the array length back at the end. Because Godot
                // allocates memory in steps of powers of two, this might end up with an array backing storage that is
                // twice as large as it needs to be. By first gathering elements into a buffer, we can tell Godot to
                // allocate exactly as much as we need, and no more.
                //
                // Note that we can't get by with simple memcpys, because `PackedStringArray` contains `GString`, which
                // does not implement `Copy`.
                //
                // Buffer size: 2 kB is enough for the performance win, without needlessly blowing up the stack size.
                // (A cursory check shows that most/all platforms use a stack size of at least 1 MB.)
                const BUFFER_SIZE_BYTES: usize = 2048;
                const BUFFER_CAPACITY: usize = const_max(
                    1,
                    BUFFER_SIZE_BYTES / size_of::<$Element>(),
                );
                let mut buf = ExtendBuffer::<_, BUFFER_CAPACITY>::default();
                while let Some(item) = iter.next() {
                    buf.push(item);
                    while !buf.is_full() {
                        if let Some(item) = iter.next() {
                            buf.push(item);
                        } else {
                            break;
                        }
                    }

                    let buf_slice = buf.drain_as_mut_slice();
                    let capacity = len + buf_slice.len();

                    // Assumption: resize does not panic. Otherwise we would leak memory here.
                    self.resize(capacity);

                    // SAFETY: Dropping the first `buf_slice.len()` items is safe, because those are exactly the ones we initialized.
                    // Writing output is safe because we just allocated `buf_slice.len()` new elements after index `len`.
                    unsafe {
                        self.move_from_slice(buf_slice.as_ptr(), len, buf_slice.len());
                    }

                    len = capacity;
                }
            }
        }

        impl_builtin_froms!($PackedArray; VariantArray => $from_array);

        impl fmt::Debug for $PackedArray {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                // Going through `Variant` because there doesn't seem to be a direct way.
                write!(f, "{:?}", self.to_variant().stringify())
            }
        }

        impl fmt::Display for $PackedArray {
            /// Formats `PackedArray` to match Godot's string representation.
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "[")?;
                for (i, elem) in self.as_slice().iter().enumerate() {
                    if i != 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{elem}")?;
                }
                write!(f, "]")
            }
        }

        unsafe impl GodotFfi for $PackedArray {
            const VARIANT_TYPE: ExtVariantType = ExtVariantType::Concrete(sys::VariantType::$VariantType);

            ffi_methods! { type sys::GDExtensionTypePtr = *mut Opaque; .. }
        }

        $crate::meta::impl_godot_as_self!($PackedArray);

        impl $crate::registry::property::Export for $PackedArray {
            fn export_hint() -> $crate::meta::PropertyHintInfo {
                // In 4.3 Godot can (and does) use type hint strings for packed arrays, see https://github.com/godotengine/godot/pull/82952.
                if sys::GdextBuild::since_api("4.3") {
                    $crate::meta::PropertyHintInfo::export_packed_array_element::<$Element>()
                } else {
                    $crate::meta::PropertyHintInfo::type_name::<$PackedArray>()
                }
            }
        }
    }
}

// Helper macro to only include specific functions in the code if the Packed*Array provides the function.
macro_rules! declare_packed_array_conversion_fns {
    (PackedByteArray) => {
        /// Returns a copy of the data converted to a `PackedFloat32Array`, where each block of 4 bytes has been converted to a 32-bit float.
        ///
        /// The size of the input array must be a multiple of 4 (size of 32-bit float). The size of the new array will be `byte_array.size() / 4`.
        ///
        /// If the original data can't be converted to 32-bit floats, the resulting data is undefined.
        pub fn to_float32_array(&self) -> PackedFloat32Array {
            self.as_inner().to_float32_array()
        }

        /// Returns a copy of the data converted to a `PackedFloat64Array`, where each block of 8 bytes has been converted to a 64-bit float.
        ///
        /// The size of the input array must be a multiple of 8 (size of 64-bit float). The size of the new array will be `byte_array.size() / 8`.
        ///
        /// If the original data can't be converted to 64-bit floats, the resulting data is undefined.
        pub fn to_float64_array(&self) -> PackedFloat64Array {
            self.as_inner().to_float64_array()
        }

        /// Returns a copy of the data converted to a `PackedInt32Array`, where each block of 4 bytes has been converted to a 32-bit integer.
        ///
        /// The size of the input array must be a multiple of 4 (size of 32-bit integer). The size of the new array will be `byte_array.size() / 4`.
        ///
        /// If the original data can't be converted to 32-bit integers, the resulting data is undefined.
        pub fn to_int32_array(&self) -> PackedInt32Array {
            self.as_inner().to_int32_array()
        }

        /// Returns a copy of the data converted to a `PackedInt64Array`, where each block of 8 bytes has been converted to a 64-bit integer.
        ///
        /// The size of the input array must be a multiple of 8 (size of 64-bit integer). The size of the new array will be `byte_array.size() / 8`.
        ///
        /// If the original data can't be converted to 64-bit integers, the resulting data is undefined.
        pub fn to_int64_array(&self) -> PackedInt64Array {
            self.as_inner().to_int64_array()
        }
    };
    ($PackedArray:ident) => {
        /// Returns a `PackedByteArray` with each value encoded as bytes.
        pub fn to_byte_array(&self) -> PackedByteArray {
            self.as_inner().to_byte_array()
        }
    };
}

impl_packed_array!(
    type_name: PackedByteArray,
    variant_type: PACKED_BYTE_ARRAY,
    element_type: u8,
    opaque_type: OpaquePackedByteArray,
    inner_type: InnerPackedByteArray,
    argument_type: i64,
    return_type: u8,
    from_array: packed_byte_array_from_array,
    operator_index: packed_byte_array_operator_index,
    operator_index_const: packed_byte_array_operator_index_const,
    trait_impls: {
        Default => packed_byte_array_construct_default;
        Clone => packed_byte_array_construct_copy;
        Drop => packed_byte_array_destroy;
        Eq => packed_byte_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedInt32Array,
    variant_type: PACKED_INT32_ARRAY,
    element_type: i32,
    opaque_type: OpaquePackedInt32Array,
    inner_type: InnerPackedInt32Array,
    argument_type: i64,
    return_type: i32,
    from_array: packed_int32_array_from_array,
    operator_index: packed_int32_array_operator_index,
    operator_index_const: packed_int32_array_operator_index_const,
    trait_impls: {
        Default => packed_int32_array_construct_default;
        Clone => packed_int32_array_construct_copy;
        Drop => packed_int32_array_destroy;
        Eq => packed_int32_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedInt64Array,
    variant_type: PACKED_INT64_ARRAY,
    element_type: i64,
    opaque_type: OpaquePackedInt64Array,
    inner_type: InnerPackedInt64Array,
    argument_type: i64,
    return_type: i64,
    from_array: packed_int64_array_from_array,
    operator_index: packed_int64_array_operator_index,
    operator_index_const: packed_int64_array_operator_index_const,
    trait_impls: {
        Default => packed_int64_array_construct_default;
        Clone => packed_int64_array_construct_copy;
        Drop => packed_int64_array_destroy;
        Eq => packed_int64_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedFloat32Array,
    variant_type: PACKED_FLOAT32_ARRAY,
    element_type: f32,
    opaque_type: OpaquePackedFloat32Array,
    inner_type: InnerPackedFloat32Array,
    argument_type: f64,
    return_type: f32,
    from_array: packed_float32_array_from_array,
    operator_index: packed_float32_array_operator_index,
    operator_index_const: packed_float32_array_operator_index_const,
    trait_impls: {
        Default => packed_float32_array_construct_default;
        Clone => packed_float32_array_construct_copy;
        Drop => packed_float32_array_destroy;
        PartialEq => packed_float32_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedFloat64Array,
    variant_type: PACKED_FLOAT64_ARRAY,
    element_type: f64,
    opaque_type: OpaquePackedFloat64Array,
    inner_type: InnerPackedFloat64Array,
    argument_type: f64,
    return_type: f64,
    from_array: packed_float64_array_from_array,
    operator_index: packed_float64_array_operator_index,
    operator_index_const: packed_float64_array_operator_index_const,
    trait_impls: {
        Default => packed_float64_array_construct_default;
        Clone => packed_float64_array_construct_copy;
        Drop => packed_float64_array_destroy;
        PartialEq => packed_float64_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedStringArray,
    variant_type: PACKED_STRING_ARRAY,
    element_type: GString,
    opaque_type: OpaquePackedStringArray,
    inner_type: InnerPackedStringArray,
    argument_type: GString,
    return_type: __GdextString,
    from_array: packed_string_array_from_array,
    operator_index: packed_string_array_operator_index,
    operator_index_const: packed_string_array_operator_index_const,
    trait_impls: {
        Default => packed_string_array_construct_default;
        Clone => packed_string_array_construct_copy;
        Drop => packed_string_array_destroy;
        Eq => packed_string_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedVector2Array,
    variant_type: PACKED_VECTOR2_ARRAY,
    element_type: Vector2,
    opaque_type: OpaquePackedVector2Array,
    inner_type: InnerPackedVector2Array,
    argument_type: Vector2,
    return_type: __GdextType,
    from_array: packed_vector2_array_from_array,
    operator_index: packed_vector2_array_operator_index,
    operator_index_const: packed_vector2_array_operator_index_const,
    trait_impls: {
        Default => packed_vector2_array_construct_default;
        Clone => packed_vector2_array_construct_copy;
        Drop => packed_vector2_array_destroy;
        PartialEq => packed_vector2_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedVector3Array,
    variant_type: PACKED_VECTOR3_ARRAY,
    element_type: Vector3,
    opaque_type: OpaquePackedVector3Array,
    inner_type: InnerPackedVector3Array,
    argument_type: Vector3,
    return_type: __GdextType,
    from_array: packed_vector3_array_from_array,
    operator_index: packed_vector3_array_operator_index,
    operator_index_const: packed_vector3_array_operator_index_const,
    trait_impls: {
        Default => packed_vector3_array_construct_default;
        Clone => packed_vector3_array_construct_copy;
        Drop => packed_vector3_array_destroy;
        PartialEq => packed_vector3_array_operator_equal;
    },
);

#[cfg(since_api = "4.3")]
impl_packed_array!(
    type_name: PackedVector4Array,
    variant_type: PACKED_VECTOR4_ARRAY,
    element_type: Vector4,
    opaque_type: OpaquePackedVector4Array,
    inner_type: InnerPackedVector4Array,
    argument_type: Vector4,
    return_type: __GdextType,
    from_array: packed_vector4_array_from_array,
    operator_index: packed_vector4_array_operator_index,
    operator_index_const: packed_vector4_array_operator_index_const,
    trait_impls: {
        Default => packed_vector4_array_construct_default;
        Clone => packed_vector4_array_construct_copy;
        Drop => packed_vector4_array_destroy;
        PartialEq => packed_vector4_array_operator_equal;
    },
);

impl_packed_array!(
    type_name: PackedColorArray,
    variant_type: PACKED_COLOR_ARRAY,
    element_type: Color,
    opaque_type: OpaquePackedColorArray,
    inner_type: InnerPackedColorArray,
    argument_type: Color,
    return_type: __GdextType,
    from_array: packed_color_array_from_array,
    operator_index: packed_color_array_operator_index,
    operator_index_const: packed_color_array_operator_index_const,
    trait_impls: {
        Default => packed_color_array_construct_default;
        Clone => packed_color_array_construct_copy;
        Drop => packed_color_array_destroy;
        PartialEq => packed_color_array_operator_equal;
    },
);

// ----------------------------------------------------------------------------------------------------------------------------------------------
// Abstractions that may later simplify the migration to generics.

macro_rules! impl_packed_trait_as_into {
    ($Type:ty, $Into:ty) => {
        impl PackedTraits for $Type {
            type ArgType = $Into;

            // fn into_packed_arg(self) -> Self::ArgType<'_> {
            fn into_packed_arg(self) -> Self::ArgType {
                self.into()
            }
        }
    };

    ($Type:ty) => {
        impl_packed_trait_as_into!($Type, $Type);
    };
}

trait PackedTraits /*: PackedArrayElement*/ {
    type ArgType;

    fn into_packed_arg(self) -> Self::ArgType;
}

impl_packed_trait_as_into!(u8, i64);
impl_packed_trait_as_into!(i32, i64);
impl_packed_trait_as_into!(i64);
impl_packed_trait_as_into!(f32, f64);
impl_packed_trait_as_into!(f64);
impl_packed_trait_as_into!(Vector2);
impl_packed_trait_as_into!(Vector3);
#[cfg(since_api = "4.3")]
impl_packed_trait_as_into!(Vector4);
impl_packed_trait_as_into!(Color);

impl<'r> PackedTraits for meta::CowArg<'r, GString> {
    type ArgType = meta::CowArg<'r, GString>;

    fn into_packed_arg(self) -> Self::ArgType {
        self
    }
}

// ----------------------------------------------------------------------------------------------------------------------------------------------
// Specific API for PackedByteArray

macro_rules! declare_encode_decode {
    // $Via could be inferred, but ensures we have the correct type expectations.
    ($Ty:ty, $bytes:literal, $encode_fn:ident, $decode_fn:ident, $Via:ty) => {
        #[doc = concat!("Encodes `", stringify!($Ty), "` as ", stringify!($bytes), " byte(s) at position `byte_offset`.")]
        ///
        /// Returns `Err` if there is not enough space left to write the value, and does nothing in that case.
        ///
        /// **Note:** byte order and encoding pattern is an implementation detail. For portable byte representation and faster encoding, use
        /// [`as_mut_slice()`][Self::as_mut_slice] and the various Rust standard APIs such as
        #[doc = concat!("[`", stringify!($Ty), "::to_be_bytes()`].")]
        pub fn $encode_fn(&mut self, byte_offset: usize, value: $Ty) -> Result<(), ()> {
            // sys::static_assert!(std::mem::size_of::<$Ty>() == $bytes); -- used for testing, can't keep enabled due to half-floats.

            if byte_offset + $bytes > self.len() {
                return Err(());
            }

            self.as_inner()
                .$encode_fn(byte_offset as i64, value as $Via);
            Ok(())
        }

        #[doc = concat!("Decodes `", stringify!($Ty), "` from ", stringify!($bytes), " byte(s) at position `byte_offset`.")]
        ///
        /// Returns `Err` if there is not enough space left to read the value. In case Godot has other error conditions for decoding, it may
        /// return zero and print an error.
        ///
        /// **Note:** byte order and encoding pattern is an implementation detail. For portable byte representation and faster decoding, use
        /// [`as_slice()`][Self::as_slice] and the various Rust standard APIs such as
        #[doc = concat!("[`", stringify!($Ty), "::from_be_bytes()`].")]
        pub fn $decode_fn(&self, byte_offset: usize) -> Result<$Ty, ()> {
            if byte_offset + $bytes > self.len() {
                return Err(());
            }

            let decoded: $Via = self.as_inner().$decode_fn(byte_offset as i64);
            Ok(decoded as $Ty)
        }
    };
}

impl PackedByteArray {
    declare_encode_decode!(u8, 1, encode_u8, decode_u8, i64);
    declare_encode_decode!(i8, 1, encode_s8, decode_s8, i64);
    declare_encode_decode!(u16, 2, encode_u16, decode_u16, i64);
    declare_encode_decode!(i16, 2, encode_s16, decode_s16, i64);
    declare_encode_decode!(u32, 4, encode_u32, decode_u32, i64);
    declare_encode_decode!(i32, 4, encode_s32, decode_s32, i64);
    declare_encode_decode!(u64, 8, encode_u64, decode_u64, i64);
    declare_encode_decode!(i64, 8, encode_s64, decode_s64, i64);
    declare_encode_decode!(f32, 2, encode_half, decode_half, f64);
    declare_encode_decode!(f32, 4, encode_float, decode_float, f64);
    declare_encode_decode!(f64, 8, encode_double, decode_double, f64);

    /// Encodes a `Variant` as bytes. Returns number of bytes written, or `Err` on encoding failure.
    ///
    /// Sufficient space must be allocated, depending on the encoded variant's size. If `allow_objects` is false, [`VariantType::OBJECT`] values
    /// are not permitted and will instead be serialized as ID-only. You should set `allow_objects` to false by default.
    pub fn encode_var(
        &mut self,
        byte_offset: usize,
        value: impl AsArg<Variant>,
        allow_objects: bool,
    ) -> Result<usize, ()> {
        meta::arg_into_ref!(value);

        let bytes_written: i64 =
            self.as_inner()
                .encode_var(byte_offset as i64, value, allow_objects);

        if bytes_written == -1 {
            Err(())
        } else {
            Ok(bytes_written as usize)
        }
    }

    /// Decodes a `Variant` from bytes and returns it, alongside the number of bytes read.
    ///
    /// Returns `Err` on decoding error. If you store legit `NIL` variants inside the byte array, use
    /// [`decode_var_allow_nil()`][Self::decode_var_allow_nil] instead.
    ///
    /// # API design
    /// Godot offers three separate methods `decode_var()`, `decode_var_size()` and `has_encoded_var()`. That comes with several problems:
    /// - `has_encoded_var()` is practically useless, because it performs the full decoding work and then throws away the variant.
    ///   `decode_var()` can do all that and more.
    /// - Both `has_encoded_var()` and `decode_var_size()` are unreliable. They don't tell whether an actual variant has been written at
    ///   the location. They interpret garbage as `Variant::nil()` and return `true` or `4`, respectively. This can very easily cause bugs
    ///   because surprisingly, some users may expect that `has_encoded_var()` returns _whether a variant has been encoded_.
    /// - The underlying C++ implementation has all the necessary information (whether a variant is there, how big it is and its value) but the
    ///   GDExtension API returns only one info at a time, requiring re-decoding on each call.
    ///
    /// godot-rust mitigates this somewhat, with the following design:
    /// - `decode_var()` treats all `NIL`s as errors. This is most often the desired behavior, and if not, `decode_var_allow_nil()` can be used.
    ///   It's also the only way to detect errors at all -- once you store legit `NIL` values, you can no longer differentiate them from garbage.
    /// - `decode_var()` returns both the decoded variant and its size. This requires two decoding runs, but only if the variant is actually
    ///   valid. Again, in many cases, a user needs the size to know where follow-up data in the buffer starts.
    /// - `decode_var_size()` and `has_encoded_var()` are not exposed.
    ///
    /// # Security
    /// You should set `allow_objects` to `false` unless you have a good reason not to. Decoding objects (e.g. coming from remote sources)
    /// can cause arbitrary code execution.
    #[doc(alias = "has_encoded_var", alias = "decode_var_size")]
    #[inline]
    pub fn decode_var(
        &self,
        byte_offset: usize,
        allow_objects: bool,
    ) -> Result<(Variant, usize), ()> {
        let variant = self
            .as_inner()
            .decode_var(byte_offset as i64, allow_objects);

        if variant.is_nil() {
            return Err(());
        }

        // It's unfortunate that this does another full decoding, but decode_var() is barely useful without also knowing the size, as it won't
        // be possible to know where to start reading any follow-up data. Furthermore, decode_var_size() often returns true when there's in fact
        // no variant written at that place, it just interprets "nil", treats it as valid, and happily returns 4 bytes.
        //
        // So we combine the two calls for the sake of convenience and to avoid accidental usage.
        let size: i64 = self
            .as_inner()
            .decode_var_size(byte_offset as i64, allow_objects);
        debug_assert_ne!(size, -1); // must not happen if we just decoded variant.

        Ok((variant, size as usize))
    }

    /// Unreliable `Variant` decoding, allowing `NIL`.
    ///
    /// <div class="warning">
    /// <p>This method is highly unreliable and will try to interpret anything into variants, even zeroed memory or random byte patterns.
    /// Only use it if you need a 1:1 equivalent of Godot's <code>decode_var()</code> and <code>decode_var_size()</code> functions.</p>
    ///
    /// <p>In the majority of cases, <a href="struct.PackedByteArray.html#method.decode_var" title="method godot::builtin::PackedByteArray::decode_var">
    /// <code>decode_var()</code></a> is the better choice, as it’s much easier to use correctly. See also its section about the rationale
    /// behind the current API design.</p>
    /// </div>
    ///
    /// Returns a tuple of two elements:
    /// 1. the decoded variant. This is [`Variant::nil()`] if a valid variant can't be decoded, or the value is of type [`VariantType::OBJECT`]
    ///    and `allow_objects` is `false`.
    /// 2. The number of bytes the variant occupies. This is `0` if running out of space, but most other failures are not recognized.
    ///
    /// # Security
    /// You should set `allow_objects` to `false` unless you have a good reason not to. Decoding objects (e.g. coming from remote sources)
    /// can cause arbitrary code execution.
    #[inline]
    pub fn decode_var_allow_nil(
        &self,
        byte_offset: usize,
        allow_objects: bool,
    ) -> (Variant, usize) {
        let byte_offset = byte_offset as i64;

        let variant = self.as_inner().decode_var(byte_offset, allow_objects);
        let decoded_size = self.as_inner().decode_var_size(byte_offset, allow_objects);
        let decoded_size = decoded_size.try_into().unwrap_or_else(|_| {
            panic!("unexpected value {decoded_size} returned from decode_var_size()")
        });

        (variant, decoded_size)
    }

    /// Returns a new `PackedByteArray`, with the data of this array compressed.
    ///
    /// On failure, Godot prints an error and this method returns `Err`. (Note that any empty results coming from Godot are mapped to `Err`
    /// in Rust.)
    pub fn compress(&self, compression_mode: CompressionMode) -> Result<PackedByteArray, ()> {
        let compressed: PackedByteArray = self.as_inner().compress(compression_mode.ord() as i64);
        populated_or_err(compressed)
    }

    /// Returns a new `PackedByteArray`, with the data of this array decompressed.
    ///
    /// Set `buffer_size` to the size of the uncompressed data.
    ///
    /// On failure, Godot prints an error and this method returns `Err`. (Note that any empty results coming from Godot are mapped to `Err`
    /// in Rust.)
    ///
    /// **Note:** Decompression is not guaranteed to work with data not compressed by Godot, for example if data compressed with the deflate
    /// compression mode lacks a checksum or header.
    pub fn decompress(
        &self,
        buffer_size: usize,
        compression_mode: CompressionMode,
    ) -> Result<PackedByteArray, ()> {
        let decompressed: PackedByteArray = self
            .as_inner()
            .decompress(buffer_size as i64, compression_mode.ord() as i64);

        populated_or_err(decompressed)
    }

    /// Returns a new `PackedByteArray`, with the data of this array decompressed, and without fixed decompression buffer.
    ///
    /// This method only accepts `BROTLI`, `GZIP`, and `DEFLATE` compression modes.
    ///
    /// This method is potentially slower than [`decompress()`][Self::decompress], as it may have to re-allocate its output buffer multiple
    /// times while decompressing, whereas `decompress()` knows its output buffer size from the beginning.
    ///
    /// GZIP has a maximal compression ratio of 1032:1, meaning it's very possible for a small compressed payload to decompress to a potentially
    /// very large output. To guard against this, you may provide a maximum size this function is allowed to allocate in bytes via
    /// `max_output_size`. Passing `None` will allow for unbounded output. If any positive value is passed, and the decompression exceeds that
    /// amount in bytes, then an error will be returned.
    ///
    /// On failure, Godot prints an error and this method returns `Err`. (Note that any empty results coming from Godot are mapped to `Err`
    /// in Rust.)
    ///
    /// **Note:** Decompression is not guaranteed to work with data not compressed by Godot, for example if data compressed with the deflate
    /// compression mode lacks a checksum or header.
    pub fn decompress_dynamic(
        &self,
        max_output_size: Option<usize>,
        compression_mode: CompressionMode,
    ) -> Result<PackedByteArray, ()> {
        let max_output_size = max_output_size.map(|i| i as i64).unwrap_or(-1);
        let decompressed: PackedByteArray = self
            .as_inner()
            .decompress_dynamic(max_output_size, compression_mode.ord() as i64);

        populated_or_err(decompressed)
    }
}

fn populated_or_err(array: PackedByteArray) -> Result<PackedByteArray, ()> {
    if array.is_empty() {
        Err(())
    } else {
        Ok(array)
    }
}

/// Helper because `usize::max()` is not const.
const fn const_max(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}
