use std::fmt;

pub struct Broken {
    value: u32,

impl Broken {
    pub fn new(value: u32) -> Self {
        Self { value
    }

    pub fn double(&self) -> u32 {
        self.value * 2
    }
}

fn dangling( {
