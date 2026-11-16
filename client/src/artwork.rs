//! Fixed-size image pools. Replacing a viewport discards pending stale work.
use crate::{load_art, Api, File, Message};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
};

#[derive(Clone, Copy)]
pub(super) enum ArtTarget {
    Thumbnail,
    Preview,
    Detail,
    Marquee,
}

pub(super) struct ArtRequest {
    pub generation: u64,
    pub id: String,
    pub file: File,
    pub cancel: Option<Arc<AtomicBool>>,
    pub target: ArtTarget,
}

type Pending = Arc<(Mutex<Option<VecDeque<ArtRequest>>>, Condvar)>;

pub(super) struct ArtworkLoader {
    pending: Pending,
}

impl ArtworkLoader {
    pub fn new(workers: usize, api: Api, cache: PathBuf, tx: mpsc::Sender<Message>) -> Self {
        let pending: Pending = Arc::new((Mutex::new(Some(VecDeque::new())), Condvar::new()));
        for _ in 0..workers {
            let (pending, api, cache, tx) =
                (pending.clone(), api.clone(), cache.clone(), tx.clone());
            thread::spawn(move || loop {
                let request = {
                    let (lock, changed) = &*pending;
                    let mut queue = lock.lock().expect("Image queue");
                    loop {
                        let Some(requests) = queue.as_mut() else {
                            return;
                        };
                        if let Some(request) = requests.pop_front() {
                            break request;
                        }
                        queue = changed.wait(queue).expect("Image queue");
                    }
                };
                let cancelled = || {
                    request
                        .cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Relaxed))
                };
                if cancelled() {
                    continue;
                }
                // DynamicImage::thumbnail keeps the original aspect ratio. The
                // imageops function instead forces the exact requested dimensions.
                let art = match request.target {
                    ArtTarget::Thumbnail => {
                        crate::load_art_sized(&api, &request.file, cache.clone(), (256, 256))
                    }
                    _ => load_art(&api, &request.file, cache.clone()),
                };
                if cancelled() {
                    continue;
                }
                let message = match request.target {
                    ArtTarget::Preview => Message::Preview(request.generation, request.id, art),
                    ArtTarget::Thumbnail => Message::Thumbnail(request.generation, request.id, art),
                    ArtTarget::Detail => Message::Art(request.generation, art),
                    ArtTarget::Marquee => Message::Marquee(request.generation, request.id, art),
                };
                if tx.send(message).is_err() {
                    return;
                }
            });
        }
        Self { pending }
    }

    pub fn replace(&self, requests: VecDeque<ArtRequest>) {
        *self.pending.0.lock().expect("Image queue") = Some(requests);
        self.pending.1.notify_all();
    }
}

impl Drop for ArtworkLoader {
    fn drop(&mut self) {
        *self.pending.0.lock().expect("Image queue") = None;
        self.pending.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use sha2::{Digest, Sha256};
    use std::{fs, io::Cursor, time::Duration};

    #[test]
    fn background_thumbnails_keep_square_landscape_and_portrait_shapes_without_cropping() {
        let cache = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let loader = ArtworkLoader::new(
            1,
            Api::new("http://127.0.0.1:1").unwrap(),
            cache.path().into(),
            tx,
        );
        for (width, height, expected) in [
            (300, 300, (256, 256)),
            (400, 200, (256, 128)),
            (200, 400, (128, 256)),
        ] {
            let original = RgbaImage::from_fn(width, height, |x, y| {
                if x < width / 4 && y < height / 4 {
                    Rgba([255, 0, 0, 255])
                } else if x >= width * 3 / 4 && y >= height * 3 / 4 {
                    Rgba([0, 0, 255, 255])
                } else {
                    Rgba([98, 212, 173, 255])
                }
            });
            let mut encoded = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(original)
                .write_to(&mut encoded, ImageFormat::Png)
                .unwrap();
            let bytes = encoded.into_inner();
            let sha256 = hex::encode(Sha256::digest(&bytes));
            fs::write(cache.path().join(format!("{sha256}.image")), &bytes).unwrap();
            loader.replace(VecDeque::from([ArtRequest {
                generation: 1,
                id: "cover".into(),
                file: File {
                    id: sha256.clone(),
                    role: "thumbnail".into(),
                    destination: "cover.png".into(),
                    size: bytes.len() as u64,
                    sha256,
                },
                cancel: None,
                target: ArtTarget::Thumbnail,
            }]));
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Message::Thumbnail(_, _, result) => {
                    let art = result.unwrap();
                    assert_eq!(art.dimensions(), expected);
                    assert_eq!(art.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
                    assert_eq!(
                        art.get_pixel(art.width() - 1, art.height() - 1),
                        &Rgba([0, 0, 255, 255])
                    );
                }
                _ => panic!("Expected a thumbnail"),
            }
        }
    }

    #[test]
    fn replacing_the_viewport_discards_all_pending_old_images() {
        let (tx, _) = mpsc::channel();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        // Zero workers makes queue replacement deterministic, without any I/O.
        let loader = ArtworkLoader::new(0, api, PathBuf::new(), tx);
        let request = |generation| ArtRequest {
            generation,
            id: String::new(),
            file: File {
                id: "a".repeat(64),
                role: "image".into(),
                destination: "image.png".into(),
                size: 0,
                sha256: "b".repeat(64),
            },
            cancel: None,
            target: ArtTarget::Preview,
        };
        loader.replace((0..50).map(|_| request(1)).collect());
        loader.replace(VecDeque::from([request(2)]));
        let guard = loader.pending.0.lock().unwrap();
        let pending = guard.as_ref().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].generation, 2);
    }
}
