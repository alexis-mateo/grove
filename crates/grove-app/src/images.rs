//! Images du protocole graphique de kitty : chaque cellule U+10EEEE affiche sa
//! part de l'image, ajustée (proportions gardées, centrée) dans la zone de son
//! placement, comme le prévoit la spécification des placeholders.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Bounds, ContentMask, Corners, Pixels, Point, RenderImage, Size, Window, point, px, size};
use grove_term::{Image, RenderCell};

/// Textures envoyées au GPU, par transmission d'image.
#[derive(Default)]
pub struct ImageCache {
    textures: HashMap<u64, Arc<RenderImage>>,
}

impl ImageCache {
    /// Dessine les cellules d'image de `cells` et les vide pour que le texte ne
    /// les dessine pas. Les textures qui ne servent plus sont libérées.
    pub fn paint(
        &mut self,
        cells: &mut [RenderCell],
        images: &HashMap<u32, Arc<Image>>,
        origin: Point<Pixels>,
        cell: Size<Pixels>,
        window: &mut Window,
    ) {
        let mut used = Vec::new();
        let same_run = |a: &RenderCell, b: &RenderCell| match (a.placeholder, b.placeholder) {
            (Some(pa), Some(pb)) => {
                a.row == b.row
                    && b.column == a.column + 1
                    && pa.image == pb.image
                    && pa.row == pb.row
                    && pb.column == pa.column + 1
            }
            _ => false,
        };
        for run in cells.chunk_by(same_run) {
            let first = &run[0];
            let Some(placeholder) = first.placeholder else { continue };
            let Some(image) = images.get(&placeholder.image) else { continue };
            let Some(texture) = self.texture(image) else { continue };
            used.push(image.generation);

            let screen = point(origin.x + cell.width * first.column as f32, origin.y + cell.height * first.row as f32);
            let placement = point(
                screen.x - cell.width * placeholder.column as f32,
                screen.y - cell.height * placeholder.row as f32,
            );
            let clip = Bounds::new(screen, size(cell.width * run.len() as f32, cell.height));
            let bounds = fit(image, placement, cell);
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                let _ = window.paint_image(bounds, Corners::default(), texture, 0, false);
            });
        }

        for cell in cells.iter_mut().filter(|c| c.placeholder.is_some()) {
            cell.c = ' ';
            cell.zerowidth = None;
        }
        let unused: Vec<u64> = self.textures.keys().filter(|g| !used.contains(g)).copied().collect();
        for generation in unused {
            if let Some(texture) = self.textures.remove(&generation) {
                let _ = window.drop_image(texture);
            }
        }
    }

    fn texture(&mut self, image: &Image) -> Option<Arc<RenderImage>> {
        if let Some(texture) = self.textures.get(&image.generation) {
            return Some(texture.clone());
        }
        // GPUI attend des pixels BGRA.
        let mut bgra = image.rgba.to_vec();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let buffer = image::RgbaImage::from_raw(image.width, image.height, bgra)?;
        let texture = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        self.textures.insert(image.generation, texture.clone());
        Some(texture)
    }
}

/// Zone de l'image à l'écran : ajustée dans le placement (colonnes × lignes,
/// déduites de sa taille si le programme ne les a pas données), centrée.
fn fit(image: &Image, placement: Point<Pixels>, cell: Size<Pixels>) -> Bounds<Pixels> {
    let (width, height) = (image.width as f32, image.height as f32);
    let (cell_width, cell_height) = (f32::from(cell.width), f32::from(cell.height));
    let columns = if image.columns > 0 { image.columns as f32 } else { (width / cell_width).ceil() };
    let rows = if image.rows > 0 { image.rows as f32 } else { (height / cell_height).ceil() };
    let (box_width, box_height) = (columns * cell_width, rows * cell_height);
    let scale = (box_width / width).min(box_height / height);
    let (drawn_width, drawn_height) = (width * scale, height * scale);
    Bounds::new(
        point(
            placement.x + px((box_width - drawn_width) / 2.0),
            placement.y + px((box_height - drawn_height) / 2.0),
        ),
        size(px(drawn_width), px(drawn_height)),
    )
}
