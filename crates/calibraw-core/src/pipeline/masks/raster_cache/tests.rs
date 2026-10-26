use super::*;

fn sample_lens() -> LensGeometryMap {
    LensGeometryMap::new(
        4,
        3,
        2,
        2,
        vec![[0.0, 0.0], [3.0, 0.0], [0.0, 2.0], [3.0, 2.0]],
    )
    .unwrap()
}

#[test]
fn concurrent_corrected_uv_preparations_reuse_one_entry() {
    let cache = CorrectedUvCache(BoundedCache::new(CONTOUR_BYTES, CORRECTED_UV_MAX_ENTRIES));
    let lens = sample_lens();
    let barrier = std::sync::Barrier::new(4);
    let vectors = std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    cache.get([4, 3], [0, 0, 4, 3], [4, 3], &lens, || {
                        barrier.wait();
                        vec![[0.25, 0.5]; 12]
                    })
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    for vector in &vectors[1..] {
        assert!(Arc::ptr_eq(&vectors[0], vector));
    }
    assert_eq!(cache.0.state.lock().unwrap().entries.len(), 1);
}

fn sample_binary(width: usize, height: usize) -> Vec<u8> {
    (0..width * height)
        .map(|index| {
            let (x, y) = (index % width, index / width);
            u8::from(x > width / 5 && x < width * 4 / 5 && y > height / 6 && y < height * 5 / 6)
        })
        .collect()
}

#[test]
fn concurrent_preparations_reuse_one_entry_per_source_and_contour() {
    let contour = ContourCache(BoundedCache::new(CONTOUR_BYTES, MAX_ENTRIES));
    let sources = SourceCache(BoundedCache::new(SOURCE_BYTES, MAX_ENTRIES));
    let binary = sample_binary(32, 24);
    let source = MaskImage::new(32, 24, binary.iter().map(|value| value * 255).collect()).unwrap();
    let barrier = std::sync::Barrier::new(8);
    let prepared = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    (
                        contour.get(binary.clone(), 32, 24).unwrap(),
                        sources.get(&source),
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    for (contour, source) in &prepared[1..] {
        assert!(Arc::ptr_eq(contour, &prepared[0].0));
        assert!(Arc::ptr_eq(source, &prepared[0].1));
    }
    assert_eq!(contour.0.state.lock().unwrap().entries.len(), 1);
    assert_eq!(sources.0.state.lock().unwrap().entries.len(), 1);
}
