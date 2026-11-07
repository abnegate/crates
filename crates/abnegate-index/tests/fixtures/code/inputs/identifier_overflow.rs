pub fn assemble(source: &Source) -> Report {
    let v0 = Kind00::load(source);
    let v1 = Kind01::load(source);
    let v2 = Kind02::load(source);
    let v3 = Kind03::load(source);
    let v4 = Kind04::load(source);
    let v5 = Kind05::load(source);
    let v6 = Kind06::load(source);
    let v7 = Kind07::load(source);
    let v8 = Kind08::load(source);
    let v9 = Kind09::load(source);
    let v10 = Kind10::load(source);
    let v11 = Kind11::load(source);
    let v12 = Kind12::load(source);
    let v13 = Kind13::load(source);
    let v14 = Kind14::load(source);
    let v15 = Kind15::load(source);
    let v16 = Kind16::load(source);
    let v17 = Kind17::load(source);
    let v18 = Kind18::load(source);
    let v19 = Kind19::load(source);
    let v20 = Kind20::load(source);
    let v21 = Kind21::load(source);
    let v22 = Kind22::load(source);
    let v23 = Kind23::load(source);
    let v24 = Kind24::load(source);
    let v25 = Kind25::load(source);
    let v26 = Kind26::load(source);
    let v27 = Kind27::load(source);
    let v28 = Kind28::load(source);
    let v29 = Kind29::load(source);
    let v30 = Kind30::load(source);
    let v31 = Kind31::load(source);
    let v32 = Kind32::load(source);
    let v33 = Kind33::load(source);
    let v34 = Kind34::load(source);
    let v35 = Kind35::load(source);
    let v36 = Kind36::load(source);
    let v37 = Kind37::load(source);
    let v38 = Kind38::load(source);
    let v39 = Kind39::load(source);
    Report::of(&[v0, v1])
}

pub fn widen(p00: u8, p01: u8, p02: u8, p03: u8, p04: u8, p05: u8, p06: u8, p07: u8, p08: u8, p09: u8, p10: u8, p11: u8, p12: u8, p13: u8, p14: u8, p15: u8, p16: u8, p17: u8, p18: u8, p19: u8, p20: u8, p21: u8, p22: u8, p23: u8, p24: u8, p25: u8, p26: u8, p27: u8, p28: u8, p29: u8, p30: u8, p31: u8, p32: u8, p33: u8, p34: u8, p35: u8, p36: u8, p37: u8, p38: u8, p39: u8) -> u32 {
    let labels = ["label-00", "label-01", "label-02", "label-03", "label-04", "label-05", "label-06", "label-07", "label-08", "label-09", "label-10", "label-11", "label-12", "label-13"];
    u32::from(p00) + u32::from(p39) + labels.len() as u32
}
