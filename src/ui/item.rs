//! The GObject shown in the grid: one per image, mirroring the bits of
//! session state a thumbnail displays.

use gtk::gdk;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ImageItem)]
    pub struct ImageItem {
        #[property(get, set)]
        id: Cell<u32>,
        #[property(get, set)]
        name: RefCell<String>,
        /// 0 = unassigned.
        #[property(get, set)]
        workspace: Cell<u32>,
        #[property(get, set, nullable)]
        texture: RefCell<Option<gdk::Texture>>,
        /// The texture is a proper thumbnail, not a stand-in.
        #[property(get, set)]
        sharp: Cell<bool>,
        #[property(get, set)]
        failed: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageItem {
        const NAME: &'static str = "OmapicImageItem";
        type Type = super::ImageItem;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ImageItem {}
}

glib::wrapper! {
    pub struct ImageItem(ObjectSubclass<imp::ImageItem>);
}

impl ImageItem {
    pub fn new(id: usize) -> Self {
        glib::Object::builder().property("id", id as u32).build()
    }
}
