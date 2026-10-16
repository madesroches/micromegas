//! Prototypes for interning runtime `&'static str` property values per callsite, measured
//! against the precomputed `..set` pattern and the dynamic path.

use std::{
    collections::HashMap,
    fmt,
    ptr::null_mut,
    sync::{
        Arc, Barrier, LazyLock, Mutex, OnceLock, RwLock,
        atomic::{AtomicPtr, Ordering},
        mpsc,
    },
    time::Instant,
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use micromegas_tracing::{
    event::EventSink,
    guards::TracingSystemGuard,
    imetric,
    levels::{LevelFilter, set_max_level},
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::{Property, PropertySet},
    spans::{ThreadBlock, ThreadStream},
};
use rustc_hash::FxHashMap;

struct BenchSink {
    discard: mpsc::Sender<Box<dyn Send>>,
}

impl EventSink for BenchSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, _: &LogMetadata) -> bool {
        true
    }
    fn on_log(&self, _: &LogMetadata, _: &[DynProperty<'_>], _: i64, _: fmt::Arguments<'_>) {}
    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, block: Arc<LogBlock>) {
        let _ = self.discard.send(Box::new(block));
    }
    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, block: Arc<MetricsBlock>) {
        let _ = self.discard.send(Box::new(block));
    }
    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}
    fn is_busy(&self) -> bool {
        false
    }
}

const REASONS: [&str; 3] = ["capacity", "ttl", "explicit"];

fn labels(n: usize) -> &'static [&'static str] {
    static L4: OnceLock<Vec<&'static str>> = OnceLock::new();
    static L64: OnceLock<Vec<&'static str>> = OnceLock::new();
    let cell = match n {
        4 => &L4,
        64 => &L64,
        _ => unreachable!(),
    };
    // Leaked like object-cache's configured prefixes, so the pointers are not literals.
    cell.get_or_init(|| {
        (0..n)
            .map(|i| &*Box::leak(format!("label{i}").into_boxed_str()))
            .collect()
    })
}

type Key<const N: usize> = [(usize, usize); N];

fn key_of<const N: usize>(values: [&'static str; N]) -> Key<N> {
    values.map(|v| (v.as_ptr() as usize, v.len()))
}

struct Entry<const N: usize> {
    key: Key<N>,
    set: &'static PropertySet,
    next: *mut Entry<N>,
}

/// Append-only list of leaked entries: lock-free hit, mutex on miss.
struct ListCache<const N: usize> {
    head: AtomicPtr<Entry<N>>,
    insert: Mutex<()>,
}

// SAFETY: entries are immutable once published and never freed.
unsafe impl<const N: usize> Sync for ListCache<N> {}

impl<const N: usize> ListCache<N> {
    const fn new() -> Self {
        Self {
            head: AtomicPtr::new(null_mut()),
            insert: Mutex::new(()),
        }
    }

    fn find(&self, key: &Key<N>) -> Option<&'static Entry<N>> {
        let mut cur = self.head.load(Ordering::Acquire);
        while !cur.is_null() {
            // SAFETY: published entries are leaked and immutable.
            let entry = unsafe { &*cur };
            if entry.key == *key {
                return Some(entry);
            }
            cur = entry.next;
        }
        None
    }

    #[inline]
    fn get(
        &self,
        values: [&'static str; N],
        build: impl FnOnce() -> &'static PropertySet,
    ) -> &'static Entry<N> {
        let key = key_of(values);
        if let Some(e) = self.find(&key) {
            return e;
        }
        let _guard = self.insert.lock().unwrap();
        if let Some(e) = self.find(&key) {
            return e;
        }
        let entry = Box::leak(Box::new(Entry {
            key,
            set: build(),
            next: self.head.load(Ordering::Relaxed),
        }));
        self.head.store(entry, Ordering::Release);
        entry
    }
}

/// Last-hit pointer in front of a `ListCache`.
struct LastCache<const N: usize> {
    last: AtomicPtr<Entry<N>>,
    list: ListCache<N>,
}

unsafe impl<const N: usize> Sync for LastCache<N> {}

impl<const N: usize> LastCache<N> {
    const fn new() -> Self {
        Self {
            last: AtomicPtr::new(null_mut()),
            list: ListCache::new(),
        }
    }

    #[inline]
    fn get(
        &self,
        values: [&'static str; N],
        build: impl FnOnce() -> &'static PropertySet,
    ) -> &'static PropertySet {
        let key = key_of(values);
        let last = self.last.load(Ordering::Acquire);
        if !last.is_null() {
            let entry = unsafe { &*last };
            if entry.key == key {
                return entry.set;
            }
        }
        let entry = self.list.get(values, build);
        self.last
            .store(entry as *const _ as *mut Entry<N>, Ordering::Release);
        entry.set
    }
}

struct Slot<const N: usize> {
    entry: AtomicPtr<Entry<N>>,
}

struct Table<const N: usize> {
    mask: usize,
    slots: Box<[Slot<N>]>,
}

/// Open-addressed table of leaked entries, republished at double size when half full: lock-free
/// hit, mutex on miss. Retired tables are leaked, so their total size stays under twice the live one.
struct HashCache<const N: usize> {
    table: AtomicPtr<Table<N>>,
    insert: Mutex<usize>,
}

unsafe impl<const N: usize> Sync for HashCache<N> {}

fn hash_key<const N: usize>(key: &Key<N>) -> usize {
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut h = rustc_hash::FxBuildHasher.build_hasher();
    key.hash(&mut h);
    h.finish() as usize
}

impl<const N: usize> Table<N> {
    fn new(capacity: usize) -> *mut Self {
        Box::leak(Box::new(Self {
            mask: capacity - 1,
            slots: (0..capacity)
                .map(|_| Slot {
                    entry: AtomicPtr::new(null_mut()),
                })
                .collect(),
        }))
    }

    #[inline]
    fn find(&self, key: &Key<N>, hash: usize) -> Option<&'static Entry<N>> {
        let mut i = hash & self.mask;
        loop {
            let p = self.slots[i].entry.load(Ordering::Acquire);
            if p.is_null() {
                return None;
            }
            let entry = unsafe { &*p };
            if entry.key == *key {
                return Some(entry);
            }
            i = (i + 1) & self.mask;
        }
    }

    fn put(&self, entry: *mut Entry<N>, hash: usize) {
        let mut i = hash & self.mask;
        while !self.slots[i].entry.load(Ordering::Relaxed).is_null() {
            i = (i + 1) & self.mask;
        }
        self.slots[i].entry.store(entry, Ordering::Release);
    }
}

impl<const N: usize> HashCache<N> {
    const fn new() -> Self {
        Self {
            table: AtomicPtr::new(null_mut()),
            insert: Mutex::new(0),
        }
    }

    #[inline]
    fn get(
        &self,
        values: [&'static str; N],
        build: impl FnOnce() -> &'static PropertySet,
    ) -> &'static PropertySet {
        let key = key_of(values);
        let hash = hash_key(&key);
        let table = self.table.load(Ordering::Acquire);
        if !table.is_null()
            && let Some(e) = unsafe { &*table }.find(&key, hash)
        {
            return e.set;
        }
        self.insert_slow(key, hash, build)
    }

    #[cold]
    fn insert_slow(
        &self,
        key: Key<N>,
        hash: usize,
        build: impl FnOnce() -> &'static PropertySet,
    ) -> &'static PropertySet {
        let mut len = self.insert.lock().unwrap();
        let mut table = self.table.load(Ordering::Acquire);
        if !table.is_null()
            && let Some(e) = unsafe { &*table }.find(&key, hash)
        {
            return e.set;
        }
        if table.is_null() || (*len + 1) * 2 > unsafe { &*table }.slots.len() {
            let capacity = if table.is_null() {
                8
            } else {
                unsafe { &*table }.slots.len() * 2
            };
            let grown = Table::<N>::new(capacity);
            if !table.is_null() {
                for slot in unsafe { &*table }.slots.iter() {
                    let p = slot.entry.load(Ordering::Relaxed);
                    if !p.is_null() {
                        unsafe { &*grown }.put(p, hash_key(&unsafe { &*p }.key));
                    }
                }
            }
            self.table.store(grown, Ordering::Release);
            table = grown;
        }
        let entry = Box::leak(Box::new(Entry {
            key,
            set: build(),
            next: null_mut(),
        }));
        unsafe { &*table }.put(entry, hash);
        *len += 1;
        entry.set
    }
}

type MapCache<const N: usize> = LazyLock<RwLock<FxHashMap<Key<N>, &'static PropertySet>>>;

#[inline]
fn map_get<const N: usize>(
    cache: &MapCache<N>,
    values: [&'static str; N],
    build: impl FnOnce() -> &'static PropertySet,
) -> &'static PropertySet {
    let key = key_of(values);
    if let Some(set) = cache.read().unwrap().get(&key) {
        return set;
    }
    *cache.write().unwrap().entry(key).or_insert_with(build)
}

type PapayaCache<const N: usize> =
    LazyLock<papaya::HashMap<Key<N>, &'static PropertySet, rustc_hash::FxBuildHasher>>;

#[inline]
fn papaya_get<const N: usize>(
    cache: &PapayaCache<N>,
    values: [&'static str; N],
    build: impl FnOnce() -> &'static PropertySet,
) -> &'static PropertySet {
    let key = key_of(values);
    let map = cache.pin();
    if let Some(set) = map.get(&key) {
        return set;
    }
    map.get_or_insert_with(key, build)
}

type SccCache<const N: usize> =
    LazyLock<scc::HashIndex<Key<N>, &'static PropertySet, rustc_hash::FxBuildHasher>>;

#[inline]
fn scc_get<const N: usize>(
    cache: &SccCache<N>,
    values: [&'static str; N],
    build: impl FnOnce() -> &'static PropertySet,
) -> &'static PropertySet {
    let key = key_of(values);
    if let Some(set) = cache.peek_with(&key, |_, v| *v) {
        return set;
    }
    let set = build();
    let _ = cache.insert_sync(key, set);
    cache.peek_with(&key, |_, v| *v).expect("just inserted")
}

type DashCache<const N: usize> =
    LazyLock<dashmap::DashMap<Key<N>, &'static PropertySet, rustc_hash::FxBuildHasher>>;

#[inline]
fn dash_get<const N: usize>(
    cache: &DashCache<N>,
    values: [&'static str; N],
    build: impl FnOnce() -> &'static PropertySet,
) -> &'static PropertySet {
    let key = key_of(values);
    if let Some(set) = cache.get(&key) {
        return *set;
    }
    *cache.entry(key).or_insert_with(build)
}

type TlsMap<K> = std::cell::RefCell<FxHashMap<K, &'static PropertySet>>;

#[inline]
fn tls_get<K: std::hash::Hash + Eq + Copy>(
    cache: &'static std::thread::LocalKey<TlsMap<K>>,
    key: K,
    build: impl FnOnce() -> &'static PropertySet,
) -> &'static PropertySet {
    cache.with(|c| {
        if let Some(set) = c.borrow().get(&key) {
            return *set;
        }
        // The global interner dedups sets across threads, so a miss costs a lookup, not a set.
        let set = build();
        c.borrow_mut().insert(key, set);
        set
    })
}

thread_local! {
    static TLS_GLOBAL: TlsMap<(usize, Key<1>)> =
        const { std::cell::RefCell::new(HashMap::with_hasher(rustc_hash::FxBuildHasher)) };
}

/// Unique per callsite: the address of a `static` the macro would emit.
macro_rules! callsite_id {
    () => {{
        static ID: u8 = 0;
        &ID as *const u8 as usize
    }};
}

fn prefix_class_set(prefix: &'static str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![
        Property::new("prefix", prefix),
        Property::new("class", "demand"),
    ])
}

fn eviction_set(prefix: &'static str, reason: &'static str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![
        Property::new("prefix", prefix),
        Property::new("reason", reason),
        Property::new("tier", "ram"),
    ])
}

fn precomputed(n: usize) -> &'static [&'static PropertySet] {
    static P4: OnceLock<Vec<&'static PropertySet>> = OnceLock::new();
    static P64: OnceLock<Vec<&'static PropertySet>> = OnceLock::new();
    let cell = if n == 4 { &P4 } else { &P64 };
    cell.get_or_init(|| labels(n).iter().map(|l| prefix_class_set(l)).collect())
}

fn precomputed_eviction() -> &'static [&'static PropertySet] {
    static P: OnceLock<Vec<&'static PropertySet>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v = vec![];
        for l in labels(4) {
            for r in REASONS {
                v.push(eviction_set(l, r));
            }
        }
        v
    })
}

type Case = fn(usize);

macro_rules! one_prop_cases {
    ($n:literal) => {
        [
            ("precomputed_set", (|i| {
                let set = precomputed($n)[i % $n];
                imetric!("name", "unit", ..set, 1);
            }) as Case),
            ("dynamic", |i| {
                let l = labels($n)[i % $n];
                imetric!("name", "unit", prefix = l, class = "demand", 1);
            }),
            ("intern_every_call", |i| {
                let set = prefix_class_set(labels($n)[i % $n]);
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_rwlock_map", |i| {
                static C: MapCache<1> = LazyLock::new(Default::default);
                let l = labels($n)[i % $n];
                let set = map_get(&C, [l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_list", |i| {
                static C: ListCache<1> = ListCache::new();
                let l = labels($n)[i % $n];
                let set = C.get([l], || prefix_class_set(l)).set;
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_hash", |i| {
                static C: HashCache<1> = HashCache::new();
                let l = labels($n)[i % $n];
                let set = C.get([l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_papaya", |i| {
                static C: PapayaCache<1> = LazyLock::new(Default::default);
                let l = labels($n)[i % $n];
                let set = papaya_get(&C, [l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_scc", |i| {
                static C: SccCache<1> = LazyLock::new(Default::default);
                let l = labels($n)[i % $n];
                let set = scc_get(&C, [l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_dashmap", |i| {
                static C: DashCache<1> = LazyLock::new(Default::default);
                let l = labels($n)[i % $n];
                let set = dash_get(&C, [l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("tls_per_callsite", |i| {
                thread_local! {
                    static C: TlsMap<Key<1>> =
                        const { std::cell::RefCell::new(HashMap::with_hasher(rustc_hash::FxBuildHasher)) };
                }
                let l = labels($n)[i % $n];
                let set = tls_get(&C, key_of([l]), || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("tls_global", |i| {
                let l = labels($n)[i % $n];
                let set = tls_get(&TLS_GLOBAL, (callsite_id!(), key_of([l])), || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_last_rotating", |i| {
                static C: LastCache<1> = LastCache::new();
                let l = labels($n)[i % $n];
                let set = C.get([l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
            ("cache_last_constant", |_| {
                static C: LastCache<1> = LastCache::new();
                let l = labels($n)[0];
                let set = C.get([l], || prefix_class_set(l));
                imetric!("name", "unit", ..set, 1);
            }),
        ]
    };
}

const ONE_PROP_4: [(&str, Case); 13] = one_prop_cases!(4);
const ONE_PROP_64: [(&str, Case); 13] = one_prop_cases!(64);

const TWO_PROPS: [(&str, Case); 6] = [
    ("cache_hash", |i| {
        static C: HashCache<2> = HashCache::new();
        let (l, r) = (labels(4)[i % 4], REASONS[(i / 4) % 3]);
        let set = C.get([l, r], || eviction_set(l, r));
        imetric!("name", "unit", ..set, 1);
    }),
    ("precomputed_set", |i| {
        let set = precomputed_eviction()[i % 12];
        imetric!("name", "unit", ..set, 1);
    }),
    ("dynamic", |i| {
        let (l, r) = (labels(4)[i % 4], REASONS[(i / 4) % 3]);
        imetric!("name", "unit", prefix = l, reason = r, tier = "ram", 1);
    }),
    ("intern_every_call", |i| {
        let (l, r) = (labels(4)[i % 4], REASONS[(i / 4) % 3]);
        let set = eviction_set(l, r);
        imetric!("name", "unit", ..set, 1);
    }),
    ("cache_rwlock_map", |i| {
        static C: MapCache<2> = LazyLock::new(Default::default);
        let (l, r) = (labels(4)[i % 4], REASONS[(i / 4) % 3]);
        let set = map_get(&C, [l, r], || eviction_set(l, r));
        imetric!("name", "unit", ..set, 1);
    }),
    ("cache_list", |i| {
        static C: ListCache<2> = ListCache::new();
        let (l, r) = (labels(4)[i % 4], REASONS[(i / 4) % 3]);
        let set = C.get([l, r], || eviction_set(l, r)).set;
        imetric!("name", "unit", ..set, 1);
    }),
];

fn bench_cases(c: &mut Criterion, group_name: &str, cases: &[(&str, Case)]) {
    let mut group = c.benchmark_group(group_name);
    for (name, case) in cases {
        let mut i = 0usize;
        group.bench_function(*name, |b| {
            b.iter(|| {
                case(i);
                i = i.wrapping_add(1);
            })
        });
    }
    group.finish();
}

fn bench_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");
    for (name, case) in ONE_PROP_4
        .iter()
        .filter(|(n, _)| !n.starts_with("cache_last") && *n != "intern_every_call")
    {
        for threads in [1u64, 8] {
            group.throughput(Throughput::Elements(threads));
            group.bench_with_input(BenchmarkId::new(*name, threads), &threads, |b, &threads| {
                b.iter_custom(|iters| {
                    let barrier = Barrier::new(threads as usize + 1);
                    std::thread::scope(|s| {
                        for t in 0..threads {
                            let barrier = &barrier;
                            s.spawn(move || {
                                barrier.wait();
                                for i in 0..iters {
                                    case((i + t) as usize);
                                }
                            });
                        }
                        barrier.wait();
                        Instant::now()
                    })
                    .elapsed()
                })
            });
        }
    }
    group.finish();
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let (discard, discarded) = mpsc::channel::<Box<dyn Send>>();
    std::thread::spawn(move || for _block in discarded {});
    let sink = Arc::new(BenchSink { discard });
    let _tracing_guard = TracingSystemGuard::new(
        10 * 1024 * 1024,
        1024 * 1024,
        10 * 1024 * 1024,
        sink,
        HashMap::new(),
        false,
    )
    .expect("init telemetry");
    set_max_level(LevelFilter::Trace);

    bench_cases(c, "one_prop_card4", &ONE_PROP_4);
    bench_cases(c, "one_prop_card64", &ONE_PROP_64);
    bench_cases(c, "two_props_card12", &TWO_PROPS);
    bench_contention(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
