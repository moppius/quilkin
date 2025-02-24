/*
 * Copyright 2021 Google LLC
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::iter::FromIterator;

use slog::Logger;

use crate::filters::{extensions, DynFilterFactory};

#[cfg(doc)]
use crate::filters::{FilterFactory, FilterRegistry};

/// A map of [`FilterFactory::name`]s to [`DynFilterFactory`] values.
pub type FilterMap = std::collections::HashMap<&'static str, DynFilterFactory>;

/// A set of filters to be registered with a [`FilterRegistry`].
#[derive(Default)]
pub struct FilterSet(FilterMap);

impl FilterSet {
    /// Returns a default set of filters which are runtime configurable and used
    /// with each endpoint.
    ///
    /// Current default filters:
    /// - [`Debug`][extensions::DebugFactory]
    /// - [`LocalRateLimit`][extensions::RateLimitFilterFactory]
    /// - [`ConcatBytes`][extensions::ConcatBytesFactory]
    /// - [`LoadBalancer`][extensions::LoadBalancerFilterFactory]
    /// - [`CaptureBytes`][extensions::CaptureBytesFactory]
    /// - [`TokenRouter`][extensions::TokenRouterFactory]
    /// - [`Compress`][extensions::CompressFactory]
    pub fn default(base: &Logger) -> Self {
        Self::default_with(base, Option::into_iter(None))
    }

    /// Returns a `FilterSet` with the filters provided through `filters` in
    /// addition to the defaults. Any filter factories provided by `filters`
    /// will override any defaults with a matching name.
    ///
    /// See [`FilterSet::default`] for a list of the current defaults.
    pub fn default_with(
        base: &Logger,
        filters: impl IntoIterator<Item = DynFilterFactory>,
    ) -> Self {
        Self::with(
            std::array::IntoIter::new([
                Box::from(extensions::DebugFactory::new(base)) as DynFilterFactory,
                Box::from(extensions::RateLimitFilterFactory::default()),
                Box::from(extensions::ConcatBytesFactory::default()),
                Box::from(extensions::LoadBalancerFilterFactory::default()),
                Box::from(extensions::CaptureBytesFactory::new(base)),
                Box::from(extensions::TokenRouterFactory::new(base)),
                Box::from(extensions::CompressFactory::new(base)),
            ])
            .chain(filters),
        )
    }

    /// Creates a new [`FilterSet`] with the set of `filter_factories` without
    /// any defaults.
    pub fn with(filters: impl IntoIterator<Item = DynFilterFactory>) -> Self {
        Self::from_iter(filters)
    }
}

impl<I: Iterator<Item = DynFilterFactory>> From<I> for FilterSet {
    fn from(iter: I) -> Self {
        Self::with(iter)
    }
}

impl FromIterator<DynFilterFactory> for FilterSet {
    fn from_iter<I: IntoIterator<Item = DynFilterFactory>>(iter: I) -> Self {
        let mut set = Self(Default::default());

        for factory in iter {
            set.0.insert(factory.name(), factory);
        }

        set
    }
}

impl IntoIterator for FilterSet {
    type IntoIter = IntoIter;
    type Item = DynFilterFactory;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            inner: self.0.into_iter(),
        }
    }
}

/// Iterator over a set of [`DynFilterFactory`]s.
pub struct IntoIter {
    inner: std::collections::hash_map::IntoIter<&'static str, DynFilterFactory>,
}

impl Iterator for IntoIter {
    type Item = DynFilterFactory;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(_, v)| v)
    }
}
