#![no_std]
use core::convert::TryFrom;
use core::slice::Iter;

pub(crate) mod common;
pub mod coords;
pub mod datetime;
pub mod gga;
pub mod gll;
pub mod modes;
pub mod rmc;

/// Source of NMEA sentence. May be navigation system or navigation sensor.
#[derive(Debug, PartialEq)]
pub enum Source {
    /// USA Global Positioning System
    GPS,
    /// Russian Federation GLONASS
    GLONASS,
    /// European Union Gallileo
    Gallileo,
    /// China's Beidou
    Beidou,
    /// Global Navigation Sattelite System. Some combination of other systems. Depends on receiver model, receiver settings, etc..
    GNSS,
}

impl TryFrom<&str> for Source {
    type Error = &'static str;

    fn try_from(from: &str) -> Result<Self, &'static str> {
        match &from[0..2] {
            "GP" => Ok(Source::GPS),
            "GL" => Ok(Source::GLONASS),
            "GA" => Ok(Source::Gallileo),
            "BD" => Ok(Source::Beidou),
            "GN" => Ok(Source::GNSS),
            _ => Err("Source is not supported"),
        }
    }
}

/// Success NMEA sentence parsing result.
/// Sentences with many null fields or sentences without valid data is also parsed and returned as None.
/// None ParseResult may be interpreted as working receiver but without valid data.
#[derive(Debug, PartialEq)]
pub enum ParseResult {
    /// Recommended Minimum Sentence for any GNSS. Typically most used.
    RMC(Option<rmc::RMC>),
    /// Geographic coordinates including altitude, GPS solution quality, DGPS usage information.
    GGA(Option<gga::GGA>),
    /// Geographic latitude ang longitude sentence with time of fix and receiver state.
    GLL(Option<gll::GLL>),
}

pub struct Parser {
    buffer: [u8; 79],
    buflen: usize,
    chksum: u8,
    expected_chksum: u8,
    parser_state: ParserState,
}

#[derive(Debug)]
enum ParserState {
    WaitStart,
    ReadUntilChkSum,
    ChkSumUpper,
    ChkSumLower,
    WaitCR,
    WaitLF,
}

pub struct ParserIterator<'a> {
    parser: &'a mut Parser,
    input: Iter<'a, u8>,
}

impl ParserIterator<'_> {
    fn new<'a>(p: &'a mut Parser, inp: &'a [u8]) -> ParserIterator<'a> {
        ParserIterator {
            parser: p,
            input: inp.iter(),
        }
    }
}

impl Iterator for ParserIterator<'_> {
    type Item = Result<ParseResult, &'static str>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(b) = self.input.next() {
            let symbol = *b;
            if let Some(r) = self.parser.parse_from_byte(symbol) {
                return Some(r);
            }
        }
        None
    }
}

impl Parser {
    pub fn new() -> Parser {
        Parser {
            buffer: [0u8; 79],
            buflen: 0,
            chksum: 0,
            expected_chksum: 0,
            parser_state: ParserState::WaitStart,
        }
    }

    pub fn parse_from_bytes<'a>(
        &'a mut self,
        input: &'a [u8],
    ) -> impl Iterator<Item = Result<ParseResult, &'static str>> + 'a {
        ParserIterator::new(self, input)
    }

    pub fn parse_from_byte(&mut self, symbol: u8) -> Option<Result<ParseResult, &'static str>> {
        let (new_state, result) = match self.parser_state {
            ParserState::WaitStart if symbol == b'$' => {
                self.buflen = 0;
                self.chksum = 0;
                (ParserState::ReadUntilChkSum, None)
            }
            ParserState::WaitStart if symbol != b'$' => (ParserState::WaitStart, None),
            ParserState::ReadUntilChkSum if symbol != b'*' => {
                if self.buffer.len() < self.buflen {
                    (
                        ParserState::WaitStart,
                        Some(Err("NMEA sentence is too long!")),
                    )
                } else {
                    self.buffer[self.buflen] = symbol;
                    self.buflen += 1;
                    self.chksum = self.chksum ^ symbol;
                    (ParserState::ReadUntilChkSum, None)
                }
            }
            ParserState::ReadUntilChkSum if symbol == b'*' => (ParserState::ChkSumUpper, None),
            ParserState::ChkSumUpper => match parse_hex_halfbyte(symbol) {
                Ok(s) => {
                    self.expected_chksum = s;
                    (ParserState::ChkSumLower, None)
                }
                Err(e) => (ParserState::WaitStart, Some(Err(e))),
            },
            ParserState::ChkSumLower => match parse_hex_halfbyte(symbol) {
                Ok(s) => {
                    if ((self.expected_chksum << 4) | s) != self.chksum {
                        (ParserState::WaitStart, Some(Err("Checksum error!")))
                    } else {
                        (ParserState::WaitCR, None)
                    }
                }
                Err(e) => (ParserState::WaitStart, Some(Err(e))),
            },
            ParserState::WaitCR if symbol == b'\r' => (ParserState::WaitLF, None),
            ParserState::WaitLF if symbol == b'\n' => {
                (ParserState::WaitStart, Some(self.parse_sentences()))
            }
            _ => (ParserState::WaitStart, Some(Err("NMEA format error!"))),
        };
        self.parser_state = new_state;
        return result;
    }

    fn parse_sentences(&self) -> Result<ParseResult, &'static str> {
        let input = from_ascii(&self.buffer[..self.buflen])?;
        let mut iter = input.split(',');
        let sentence_field = iter
            .next()
            .ok_or("Sentence type not found but mandatory!")?;
        if sentence_field.len() < 5 {
            return Err("Sentence field is too small. Must be 5 chars at least!");
        }
        let source = Source::try_from(sentence_field)?;
        match &sentence_field[2..5] {
            "RMC" => Ok(ParseResult::RMC(rmc::RMC::parse(source, &mut iter)?)),
            "GGA" => Ok(ParseResult::GGA(gga::GGA::parse(source, &mut iter)?)),
            "GLL" => Ok(ParseResult::GLL(gll::GLL::parse(source, &mut iter)?)),
            _ => Err("Unsupported sentence type"),
        }
    }
}

fn from_ascii(bytes: &[u8]) -> Result<&str, &'static str> {
    if bytes.iter().all(|b| *b < 128) {
        Ok(unsafe { core::str::from_utf8_unchecked(bytes) })
    } else {
        Err("Not an ascii!")
    }
}

fn parse_hex_halfbyte(symbol: u8) -> Result<u8, &'static str> {
    if symbol >= b'0' && symbol <= b'9' {
        return Ok(symbol - b'0');
    }
    if symbol >= b'A' && symbol <= b'F' {
        return Ok(symbol - b'A' + 10);
    }
    Err("Invalid HEX character.")
}
