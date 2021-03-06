use ::std::io::{ErrorKind, BufReader, Result};
use std::env;
use std::collections::HashMap;
use std::collections::BTreeMap;
use std::vec;

const NUM_SPEED:usize = 512;
const MAX_MAX: i32 = 16384;
const MIN_MAX: i32 = 0x200;
const BLEND_FIXED_POINT_PRECISION:i8=16;
type Prob=i32;
#[derive(Clone, Copy, Debug)]
struct Speed {
    inc: Prob,
    max: Prob,
    algo: u8,
}

impl Default for Speed {
    fn default() -> Speed {
        Speed{
            inc:1,
            max:MIN_MAX,
            algo: 0,
        }
    }
}
impl Speed {
    fn inc(&mut self) {
        if self.max == MIN_MAX {
            self.max += 0x200;
        } else {
            self.max += 0x400;
        }
        if self.max > MAX_MAX {
            self.inc *= 3;
            self.inc = self.inc / 2 + (self.inc & 1);
            self.max = MIN_MAX;
        }
    }
}

const CDF_MAX: Prob = 32767;
#[derive(Clone,Copy)]
struct BlendCDF16([Prob;16]);
impl Default for BlendCDF16 {
    fn default() -> Self {
        BlendCDF16(
            [0;16]
        )
    }
}
pub fn to_blend_lut(symbol: u8) -> [Prob;16] {
    const DEL: Prob = CDF_MAX - 16;
    static CDF_SELECTOR : [[Prob;16];16] = [
        [DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,0,DEL,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,0,0,DEL,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,0,0,0,DEL,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,0,0,0,0,DEL,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,0,0,0,0,0,DEL,DEL as Prob],
        [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,DEL as Prob]];
    CDF_SELECTOR[symbol as usize]
}
impl BlendCDF16 {
    fn max(&self) -> Prob {
        CDF_MAX as Prob
    }

    fn pdf(&self, symbol: u8) -> Prob {
        let mut ret = if symbol == 0 {
           self.cdf(0)
        } else {
           self.cdf(symbol) - self.cdf(symbol - 1)
        };
        if ret == 0 {
            ret = self.cdf(symbol);
            if symbol != 0 {
               ret -= self.cdf(symbol - 1);
            }
        }
        assert!(ret != 0);
        ret
    }
    fn cdf(&self, symbol: u8) -> Prob {
        match symbol {
            15 => self.max(),
            _ => {
                // We want self.cdf[15] to be normalized to CDF_MAX, so take the difference to
                // be the latent bias term coming from a uniform distribution.
                let bias = CDF_MAX - self.0[15];
                debug_assert!(bias >= 16);
                self.0[symbol as usize] as Prob + ((i32::from(bias) * (i32::from(symbol + 1))) >> 4) as Prob
            }
        }
    }
    fn blend_internal(&mut self, to_blend: [Prob;16], mix_rate: Prob) {
        self.0 = mul_blend(self.0, to_blend, mix_rate, /*1(self.count & 0xf)*/0x0 << (BLEND_FIXED_POINT_PRECISION - 4));
        if self.0[15] < (CDF_MAX - 16) - (self.0[15] >> 1) {
            for i in 0..16 {
                self.0[i] += self.0[i] >> 1;
            }
        }
        assert!(self.0[15] <= CDF_MAX - 16);

    }

    fn blend(&mut self, nibble: u8, speed:Speed) {
        let old_self = *self;
        let to_blend = to_blend_lut(nibble);
        let mr = speed.inc;
        self.blend_internal(to_blend, mr);
        // Reduce the weight of bias in the first few iterations.
    }
}



#[derive(Clone,Copy)]
struct FrequentistCDF16([Prob;16]);

impl Default for FrequentistCDF16 {
    fn default() -> Self {
        FrequentistCDF16(
            [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]
        )
    }
}

impl FrequentistCDF16 {
    fn max(&self) -> Prob {
        self.0[15]
    }
    fn cdf(&self, nibble: u8) -> Prob {
       self.0[nibble as usize]
    }
    fn pdf(&self, nibble: u8) -> Prob {
        if nibble == 0 {
            self.0[0]
        } else {
            self.0[nibble as usize] - self.0[nibble as usize-1]
        }
    }
    fn assert_ok(&self, _old: [i32;16]) {
        let mut last = 0;
        for item in self.0.iter() {
            assert!(*item != last);
            last = *item;
        }
    }
    fn blend(&mut self, nibble: u8, speed:Speed) {
        let old_self = *self;
        for i in nibble as usize..16 {
            self.0[i] += speed.inc;
        }
        if self.max() >= speed.max {
            for (index, item) in self.0.iter_mut().enumerate() {
                let cdf_bias = 1 + index as Prob;
                *item = *item + cdf_bias - (*item  + cdf_bias) / 2;
            }
        }
        self.assert_ok(old_self.0);
    }
}
pub fn mul_blend(baseline: [Prob;16], to_blend: [Prob;16], blend : Prob, bias : Prob) -> [Prob;16] {
    let blend = i64::from(if blend > 16384 {16384} else {blend});
    let bias = i64::from(bias);
    const SCALE :i64 = 1 << BLEND_FIXED_POINT_PRECISION;
    let mut epi32:[i64;8] = [i64::from(to_blend[0]),
                             i64::from(to_blend[1]),
                             i64::from(to_blend[2]),
                             i64::from(to_blend[3]),
                             i64::from(to_blend[4]),
                             i64::from(to_blend[5]),
                             i64::from(to_blend[6]),
                             i64::from(to_blend[7])];
    let scale_minus_blend = SCALE - blend;
    for i in 0..8 {
        epi32[i] *= blend;
        epi32[i] += i64::from(baseline[i]) * scale_minus_blend + bias;
        epi32[i] >>= BLEND_FIXED_POINT_PRECISION;
    }
    let mut retval : [Prob;16] =[epi32[0] as Prob,
                                 epi32[1] as Prob,
                                 epi32[2] as Prob,
                                 epi32[3] as Prob,
                                 epi32[4] as Prob,
                                 epi32[5] as Prob,
                                 epi32[6] as Prob,
                                 epi32[7] as Prob,
                                 0,0,0,0,0,0,0,0];
    let mut epi32:[i64;8] = [i64::from(to_blend[8]),
                             i64::from(to_blend[9]),
                             i64::from(to_blend[10]),
                             i64::from(to_blend[11]),
                             i64::from(to_blend[12]),
                             i64::from(to_blend[13]),
                             i64::from(to_blend[14]),
                             i64::from(to_blend[15])];
    for i in 8..16 {
        epi32[i - 8] *= blend;
        epi32[i - 8] += i64::from(baseline[i]) * scale_minus_blend + bias;
        retval[i] = (epi32[i - 8] >> BLEND_FIXED_POINT_PRECISION) as Prob;
    }
    retval
}


#[derive(Clone,Copy)]
enum CombinationCDF16 {
  Freq(FrequentistCDF16),
  Blend(BlendCDF16),
}
impl Default for CombinationCDF16 {
  fn default() -> Self {
     CombinationCDF16::Freq(FrequentistCDF16::default())
  }
}

impl CombinationCDF16 {
    fn max(&self) -> Prob {
        match *self {
           CombinationCDF16::Freq(x) => x.max(),
           CombinationCDF16::Blend(x) => x.max(),
        }
    }
    fn pdf(&self, nibble: u8) -> Prob {
        match *self {
           CombinationCDF16::Freq(x) => x.pdf(nibble),
           CombinationCDF16::Blend(x) => x.pdf(nibble),
        }
    }
    fn cdf(&self, nibble: u8) -> Prob {
        match *self {
           CombinationCDF16::Freq(x) => x.cdf(nibble),
           CombinationCDF16::Blend(x) => x.cdf(nibble),
        }
    }
    fn blend(&mut self, nibble: u8, speed:Speed) {
       if speed.algo == 0 {
          if let CombinationCDF16::Freq(_x) = *self {
          } else {
             *self = CombinationCDF16::Freq(FrequentistCDF16::default());
          }
       }
       if speed.algo == 1 {
          if let CombinationCDF16::Blend(_x) = *self {
          } else {
             *self = CombinationCDF16::Blend(BlendCDF16::default());
          }          
       }
       match *self {
           CombinationCDF16::Freq(ref mut x) => x.blend(nibble, speed),
           CombinationCDF16::Blend(ref mut x) => x.blend(nibble, speed),
       }
    }
}


//type DefaultCDF16 = FrequentistCDF16;
//type DefaultCDF16 = BlendCDF16;
type DefaultCDF16 = CombinationCDF16;



fn determine_cost(cdf: &DefaultCDF16,
                  nibble: u8) -> f64 {
    let pdf = cdf.pdf(nibble);
    assert!(pdf != 0);
    let prob = (pdf as f64) / (cdf.max() as f64);
    return -prob.log2()
}

fn eval_stream<Reader:std::io::BufRead>(
    r :&mut Reader,
    speed: Option<Speed>,
    use_preselected: bool, 
    is_hex: bool
) -> Result<f64> {
    let mut sub_streams = HashMap::<u64, vec::Vec<u8>>::new();
    let mut best_speed = BTreeMap::<(u64, bool), (Speed, f64)>::new();
    let mut buffer = String::new();
    //let mut stream_state = HashMap::<(u64, u8), DefaultCDF16>::new();
    let mut cost: f64 = 0.0;
    loop {
        buffer.clear();
        match r.read_line(&mut buffer) {
            Err(e) => {
                if e.kind() == ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            },
            Ok(val) => {
                if val == 0 || val == 1{
                    break;
                }
                let line = buffer.trim().to_string();
                let mut prior_val: Vec<String> = if let Some(_) = line.find(",") {
                     line.split(',').map(|s| s.to_string()).collect()
                } else {
                     line.split(' ').map(|s| s.to_string()).collect()
                };
                let prior = if is_hex {
                    match u64::from_str_radix(&prior_val[0], 16) {
                        Err(_) => return Err(std::io::Error::new(ErrorKind::InvalidData,prior_val[0].clone())),
                        Ok(val) => val, 
                    }
                } else {
                    match prior_val[0].parse::<u64>() {
                        Err(_) => return Err(std::io::Error::new(ErrorKind::InvalidData,prior_val[0].clone())),
                        Ok(val) => val,
                    }
                };
                    
                let val = if is_hex {
                    match u8::from_str_radix(&prior_val[1], 16) {
                        Err(_) => return Err(std::io::Error::new(ErrorKind::InvalidData,prior_val[1].clone())),
                        Ok(val) => val,
                    }                    
                } else {
                    match prior_val[1].parse::<u8>() {
                        Err(_) => return Err(std::io::Error::new(ErrorKind::InvalidData, prior_val[1].clone())),
                        Ok(val) => val,
                    }
                };
                let mut prior_stream = &mut sub_streams.entry(prior).or_insert(vec::Vec::<u8>::new());
                prior_stream.push(val);
            }
        }
    }
    let specified_speed = match speed {
        Some(s) => [s],
        None => [Speed::default()],
    };
    let mut trial_speeds = [Speed::default(); NUM_SPEED];
    let mut cur_speed = Speed::default();
    for val in trial_speeds.iter_mut() {
        *val = cur_speed;
        cur_speed.inc();
    }
    let preselected_speeds = [
        Speed { inc: 0, max: 32, algo: 0, },
        Speed { inc: 1, max: 32, algo: 0, },
        Speed { inc: 1, max: 64, algo: 0, },
        Speed { inc: 1, max: 96, algo: 0, },
        Speed { inc: 1, max: 128, algo: 0, },
        Speed { inc: 1, max: 256, algo: 0, },
        Speed { inc: 1, max: 384, algo: 0, },
        Speed { inc: 1, max: 512, algo: 0, },
        Speed { inc: 1, max: 1024, algo: 0, },
        Speed { inc: 1, max: 2048, algo: 0, },
        Speed { inc: 1, max: 4096, algo: 0, },
        Speed { inc: 1, max: 8192, algo: 0, },
        Speed { inc: 1, max: 16384, algo: 0, },
        Speed { inc: 2, max: 512, algo: 0, },
        Speed { inc: 2, max: 1024, algo: 0, },
        Speed { inc: 2, max: 2048, algo: 0, },
        Speed { inc: 2, max: 4096, algo: 0, },
        Speed { inc: 3, max: 512, algo: 0, },
        Speed { inc: 3, max: 2048, algo: 0, },
        Speed { inc: 4, max: 512, algo: 0, },
        Speed { inc: 4, max: 2048, algo: 0, },
        Speed { inc: 5, max: 8192, algo: 0, },
        Speed { inc: 6, max: 2048, algo: 0, },
        Speed { inc: 6, max: 4096, algo: 0, },
        Speed { inc: 10, max: 2048, algo: 0, },
        Speed { inc: 12, max: 4096, algo: 0, },
        Speed { inc: 16, max: 8192, algo: 0, },
        Speed { inc: 24, max: 16384, algo: 0, },
        Speed { inc: 32, max: 16384, algo: 0,},
        Speed { inc: 48, max: 16384, algo: 0,},
        Speed { inc: 64, max: 16384, algo: 0,},
        Speed { inc: 96, max: 16384, algo: 0,},
        Speed { inc: 128, max: 16384, algo: 0,},
        Speed { inc: 192, max: 16384, algo: 0,},
        Speed { inc: 256, max: 16384, algo: 0,},
        Speed { inc: 320, max: 16384, algo: 0,},
        Speed { inc: 384, max: 16384, algo: 0,},
        Speed { inc: 512, max: 16384, algo: 0,},
        Speed { inc: 768, max: 16384, algo: 0,},
        Speed { inc: 1024, max: 16384, algo: 0,},
        
        //Speed {  inc: 1, max: 512, algo: 1,},
        Speed { inc: 60, max: 16384, algo: 1, },
        Speed { inc: 100, max: 16384, algo: 1, },
        Speed { inc: 140, max: 16384, algo: 1, },
        Speed { inc: 160, max: 16384, algo: 1, },
        Speed { inc: 180, max: 16384, algo: 1, },
        Speed { inc: 195, max: 16384, algo: 1, },
        Speed { inc: 210, max: 16384, algo: 1, },
        Speed { inc: 230, max: 16384, algo: 1, },
        Speed { inc: 260, max: 16384, algo: 1, },
        Speed { inc: 280, max: 16384, algo: 1, },
        Speed { inc: 300, max: 16384, algo: 1, },
        Speed { inc: 315, max: 16384, algo: 1, },
        Speed { inc: 473, max: 16384, algo: 1, },
        Speed { inc: 560, max: 16384, algo: 1, },
        Speed { inc: 600, max: 16384, algo: 1, },
        Speed { inc: 640, max: 16384, algo: 1, },
        Speed { inc: 680, max: 16384, algo: 1, },
        Speed { inc: 710, max: 16384, algo: 1, },
        Speed { inc: 850, max: 16384, algo: 1, },
        Speed { inc: 1065, max: 16384, algo: 1, },
        Speed { inc: 1200, max: 16384, algo: 1, },
        Speed { inc: 1600, max: 16384, algo: 1, },
    ];
    let speed_choice = match speed {
        Some(_) => &specified_speed[..],
        None => if use_preselected {
            &preselected_speeds[..]
        } else {
            &trial_speeds[..]
        },
    };
    for (&prior, sub_stream) in sub_streams.iter() {
        let mut best_cost_high: Option<f64> = None;
        let mut best_speed_high = Speed::default();
        let mut best_speed_low = Speed::default();
        let mut best_cost_low: Option<f64> = None;
        for cur_speed in speed_choice.iter() {
            let mut cdf0 = DefaultCDF16::default();
            let mut cdf1a = [
                DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(),
                DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(),
                DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(),
                DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(), DefaultCDF16::default(),
                ];
                            
            let mut cur_cost_high: f64 = 0.0;
            let mut cur_cost_low: f64 = 0.0;
            for val in sub_stream.iter() {
                let val_nibbles = (val >> 4, val & 0xf);
                {
                    cur_cost_high += determine_cost(&cdf0, val_nibbles.0);
                    cdf0.blend(val_nibbles.0, *cur_speed);
                }
                {
                    let cdf1 = &mut cdf1a[val_nibbles.0 as usize];
                    cur_cost_low += determine_cost(cdf1, val_nibbles.1);
                    cdf1.blend(val_nibbles.1, *cur_speed);
                }
            }
            best_cost_high = match best_cost_high.clone() {
                None => {
                    best_speed_high = *cur_speed;
                    Some(cur_cost_high)
                },
                Some(bc) => Some(if bc > cur_cost_high {
                    best_speed_high = *cur_speed;
                    cur_cost_high
                } else {bc}),
            };
            best_cost_low = match best_cost_low.clone() {
                None => {
                    best_speed_low = *cur_speed;
                    Some(cur_cost_low)
                },
                Some(bc) => Some(if bc > cur_cost_low {
                    best_speed_low = *cur_speed;
                    cur_cost_low
                } else {bc}),
            };
        }
        best_speed.insert((prior, false), (best_speed_low, best_cost_low.unwrap()));
        best_speed.insert((prior, true), (best_speed_high, best_cost_high.unwrap()));
        cost += best_cost_high.unwrap();
        cost += best_cost_low.unwrap();
    }
    for (prior, val) in best_speed.iter() {
        print!("{:?} {:?} cost: {}\n", prior, val.0, val.1);
    }
    
    Ok(cost)
}


fn main() {
    let stdin = std::io::stdin();
    let stdin = stdin.lock();
    let mut buffered_in = BufReader::new(stdin);
    let mut speed: Option<Speed> = None;
    let use_preselected = env::args_os().len() == 2;
    if use_preselected {
        print!("arg count == 1 Using preselected list\n");
    }
    if env::args_os().len() > 2 {
        let mut first:Prob = 0;
        let mut second:Prob = 0;
        for argument in env::args().skip(1) {
            first = argument.parse::<Prob>().unwrap();
            break;
            //speed = Some(argument.parse::<Speed>().unwrap());
        }
        for argument in env::args().skip(2) {
            second = argument.parse::<Prob>().unwrap();
            break;
            //speed = Some(argument.parse::<Speed>().unwrap());
        }
        speed = Some(Speed{inc:first, max:second, algo:0 });
    }
    let cost = eval_stream(&mut buffered_in, speed, use_preselected, true).unwrap();
    println!("{} bytes; {} bits", ((cost + 0.99) as u64) as f64 / 8.0, (cost + 0.99) as u64);
}
