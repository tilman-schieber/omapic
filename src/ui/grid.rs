//! Thumbnail cells: picture + workspace badge, hover, drag-and-drop.

use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::App;
use super::item::ImageItem;

/// Logical size of a thumbnail cell.
pub const CELL: i32 = 168;

fn item_of(list_item: &glib::WeakRef<gtk::ListItem>) -> Option<ImageItem> {
    list_item.upgrade()?.item().and_downcast::<ImageItem>()
}

pub fn factory(app: &Rc<App>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    let weak: Weak<App> = Rc::downgrade(app);
    factory.connect_setup(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().unwrap();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .width_request(CELL)
            .height_request(CELL)
            .build();
        let badge = gtk::Label::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .css_classes(["badge"])
            .build();
        let broken = gtk::Label::builder().label("unreadable").css_classes(["broken"]).build();
        let cell = gtk::Overlay::builder().child(&picture).css_classes(["cell"]).build();
        cell.add_overlay(&badge);
        cell.add_overlay(&broken);
        list_item.set_child(Some(&cell));

        let item = list_item.property_expression("item");
        item.chain_property::<ImageItem>("texture").bind(&picture, "paintable", gtk::Widget::NONE);
        item.chain_property::<ImageItem>("failed").bind(&broken, "visible", gtk::Widget::NONE);
        let workspace = item.chain_property::<ImageItem>("workspace");
        workspace
            .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, ws: u32| ws.to_string()))
            .bind(&badge, "label", gtk::Widget::NONE);
        workspace
            .chain_closure::<bool>(glib::closure!(|_: Option<glib::Object>, ws: u32| ws != 0))
            .bind(&badge, "visible", gtk::Widget::NONE);

        // Real pointer motion only: content scrolling under a resting pointer
        // (keyboard navigation) must not steal the preview.
        let motion = gtk::EventControllerMotion::new();
        let (app, li) = (weak.clone(), list_item.downgrade());
        motion.connect_motion(move |_, _, _| {
            if let (Some(app), Some(item)) = (app.upgrade(), item_of(&li)) {
                app.hover(Some(item.id() as usize));
            }
        });
        cell.add_controller(motion);

        let drag = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        let li = list_item.downgrade();
        drag.connect_prepare(move |_, _, _| {
            let item = item_of(&li)?;
            Some(gdk::ContentProvider::for_value(&item.id().to_value()))
        });
        let li = list_item.downgrade();
        drag.connect_drag_begin(move |source, _| {
            if let Some(texture) = item_of(&li).and_then(|i| i.texture()) {
                source.set_icon(Some(&texture), 0, 0);
            }
        });
        cell.add_controller(drag);

        let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        let (app, li) = (weak.clone(), list_item.downgrade());
        drop.connect_drop(move |_, value, _, _| {
            let (Some(app), Some(target), Ok(dragged)) = (app.upgrade(), item_of(&li), value.get::<u32>())
            else {
                return false;
            };
            app.reorder_by_drop(dragged as usize, target.id() as usize);
            true
        });
        cell.add_controller(drop);
    });

    let weak = Rc::downgrade(app);
    factory.connect_bind(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().unwrap();
        if let (Some(app), Some(item)) = (weak.upgrade(), list_item.item().and_downcast::<ImageItem>()) {
            app.cell_bound(&item, list_item.position() as usize);
        }
    });
    let weak = Rc::downgrade(app);
    factory.connect_unbind(move |_, object| {
        let item = object.downcast_ref::<gtk::ListItem>().and_then(|li| li.item());
        if let (Some(app), Some(item)) = (weak.upgrade(), item.and_downcast::<ImageItem>()) {
            app.cell_unbound(&item);
        }
    });
    factory
}
