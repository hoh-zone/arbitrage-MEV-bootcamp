use std::{fmt::Debug, sync::Arc};

pub struct ObjectPool<T> {
    pub objects: Vec<Arc<T>>,
}

impl<T> ObjectPool<T> {
    pub fn new<F>(num_objects: usize, init_fn: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        let init_fn = Arc::new(init_fn);
        let mut handles = Vec::with_capacity(num_objects);

        // Spawn threads to initialize objects in parallel
        for _ in 0..num_objects {
            let init_fn = init_fn.clone();
            handles.push(std::thread::spawn(move || Arc::new((init_fn)())));
        }

        // Collect results from all threads
        let objects = handles.into_iter().map(|handle| handle.join().unwrap()).collect();

        Self { objects }
    }

    // get the one with the least refcount
    pub fn get(&self) -> Arc<T> {
        self.objects
            .iter()
            .min_by_key(|obj| Arc::strong_count(obj))
            .unwrap()
            .clone()
    }
}

impl<T> Debug for ObjectPool<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.objects.len();
        let ref_counts: Vec<_> = self.objects.iter().map(|obj| Arc::strong_count(obj)).collect();
        let max_ref = ref_counts.iter().max().unwrap_or(&0);
        let min_ref = ref_counts.iter().min().unwrap_or(&0);

        write!(f, "ObjectPool(len={}, max_ref={}, min_ref={}", len, max_ref, min_ref)?;

        if len < 32 {
            write!(f, ", ref_counts={:?}", ref_counts)?;
        }

        write!(f, ")")
    }
}
