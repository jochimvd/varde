mod idle;
mod privacy;

pub struct Widgets {
    pub idle: gtk::Button,
    pub privacy: gtk::Box,
}

pub fn widgets() -> Widgets {
    Widgets {
        idle: idle::widget(),
        privacy: privacy::widget(),
    }
}
