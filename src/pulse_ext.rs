use pulse::error::PAErr;

pub trait PulseSimpleExt {
    fn read16(&self, buf: &mut [i16]) -> Result<(), PAErr>;
    fn write16(&self, buf: &[i16]) -> Result<(), PAErr>;
}

impl PulseSimpleExt for simple_pulse::Simple {
    fn read16(&self, buf: &mut [i16]) -> Result<(), PAErr> {
        let (_, buf, _) = unsafe { buf.align_to_mut::<u8>() };
        self.read(buf)
    }

    fn write16(&self, buf: &[i16]) -> Result<(), PAErr> {
        let (_, buf, _) = unsafe { buf.align_to::<u8>() };
        self.write(buf)
    }
}
