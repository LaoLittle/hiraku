use rhai::{Array, Dynamic, ImmutableString, Map, plugin::*};

#[allow(non_snake_case)]
#[export_module]
pub mod Ui {
    use super::*;

    pub fn screen(mut options: Map, children: Array) -> Map {
        options.insert("children".into(), Dynamic::from_array(children));
        options
    }

    pub fn image(texture: ImmutableString, mut options: Map) -> Map {
        options.insert("type".into(), Dynamic::from("image"));
        options.insert("texture".into(), Dynamic::from(texture));
        options
    }

    pub fn image_button(texture: ImmutableString, value: Dynamic, mut options: Map) -> Map {
        options.insert("type".into(), Dynamic::from("image_button"));
        options.insert("texture".into(), Dynamic::from(texture));
        options.insert("value".into(), value);
        options
    }

    pub fn text(text: ImmutableString, mut options: Map) -> Map {
        options.insert("type".into(), Dynamic::from("text"));
        options.insert("text".into(), Dynamic::from(text));
        options
    }

    pub fn button(text: ImmutableString, value: Dynamic, mut options: Map) -> Map {
        options.insert("type".into(), Dynamic::from("button"));
        options.insert("text".into(), Dynamic::from(text));
        options.insert("value".into(), value);
        options
    }

    pub fn vbox(mut options: Map, children: Array) -> Map {
        options.insert("type".into(), Dynamic::from("vbox"));
        options.insert("children".into(), Dynamic::from_array(children));
        options
    }

    pub fn hbox(mut options: Map, children: Array) -> Map {
        options.insert("type".into(), Dynamic::from("hbox"));
        options.insert("children".into(), Dynamic::from_array(children));
        options
    }
}
