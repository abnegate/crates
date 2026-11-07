pub const A: u8 = 1;
pub const B: u8 = 2;
const C: u8 = 3;
static D: u8 = 4;
pub type Small = u8;
type Tiny = u16;
pub struct Unit;
struct Marker;
pub enum Flag { On, Off }
const E: &str = "e";

pub fn named_helper() -> u8 {
    A + B
}

pub const AFTER_FUNCTION: u8 = 9;
const ALSO_AFTER: u8 = 10;
