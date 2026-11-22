//! Presentation-only Maidenhead locations and approximate great-circle distances.
//! Grid arithmetic follows WSJT-X lib/grid2deg.f90 (GPL-3.0-or-later,
//! WSJT development team); use east-positive longitude and actual cell centers.
use crate::message_kind::MessageKind;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Location {
    pub grid: String,
    pub latitude: f64,
    pub longitude: f64,
}

impl Location {
    pub fn parse(grid: &str) -> Option<Self> {
        let grid = grid.to_ascii_uppercase();
        let b = grid.as_bytes();
        if !matches!(b.len(), 4 | 6)
            || !b[..2].iter().all(|c| (b'A'..=b'R').contains(c))
            || !b[2..4].iter().all(u8::is_ascii_digit)
            || (b.len() == 6 && !b[4..].iter().all(|c| (b'A'..=b'X').contains(c)))
        {
            return None;
        }
        let mut longitude = -180.0 + 20.0 * (b[0] - b'A') as f64 + 2.0 * (b[2] - b'0') as f64;
        let mut latitude = -90.0 + 10.0 * (b[1] - b'A') as f64 + (b[3] - b'0') as f64;
        if b.len() == 6 {
            longitude += ((b[4] - b'A') as f64 + 0.5) / 12.0;
            latitude += ((b[5] - b'A') as f64 + 0.5) / 24.0;
        } else {
            longitude += 1.0;
            latitude += 0.5;
        }
        Some(Self {
            grid,
            latitude,
            longitude,
        })
    }

    pub fn distance_km(&self, other: &Self) -> f64 {
        let a = ((other.latitude - self.latitude).to_radians() / 2.0)
            .sin()
            .powi(2)
            + self.latitude.to_radians().cos()
                * other.latitude.to_radians().cos()
                * ((other.longitude - self.longitude).to_radians() / 2.0)
                    .sin()
                    .powi(2);
        12_742.0 * a.clamp(0.0, 1.0).sqrt().asin()
    }
}

/// Only ordinary CQ/locator messages supply a location, belonging to the sender.
pub fn decoded_location(text: &str, kind: MessageKind) -> Option<Location> {
    if !matches!(kind, MessageKind::Cq | MessageKind::Reply) {
        return None;
    }
    Location::parse(text.split_whitespace().last()?)
}

pub fn description(location: Option<&Location>, mine: &str) -> String {
    let Some(location) = location else {
        return "Grid / distance unknown".into();
    };
    let distance = Location::parse(mine)
        .map(|home| format!(" · ≈{:.0} km", home.distance_km(location)))
        .unwrap_or_else(|| " · set My grid for distance".into());
    format!(
        "{} · {:.3}°, {:.3}°{}",
        location.grid, location.latitude, location.longitude, distance
    )
}

#[derive(Clone, Copy)]
pub enum CardSort {
    Signal,
    State,
    Distance,
}

pub fn distance_key(location: Option<&Location>, mine: &str) -> f64 {
    location
        .zip(Location::parse(mine))
        .map(|(l, home)| l.distance_km(&home))
        .unwrap_or(f64::INFINITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centers_validation_and_distance() {
        let en = Location::parse("EN37").unwrap();
        assert_eq!((en.latitude, en.longitude), (47.5, -93.0));
        let six = Location::parse("en37aa").unwrap();
        assert!((six.latitude - 47.020833333).abs() < 1e-8);
        assert!((six.longitude + 93.958333333).abs() < 1e-8);
        for bad in ["", "SS00", "EN3", "EN37YY", "éN37", "EN3700"] {
            assert!(Location::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(en.distance_km(&en), 0.0);
        let london = Location::parse("IO91").unwrap();
        assert!((en.distance_km(&london) - 6300.0).abs() < 150.0);
        assert!(decoded_location("K1ABC W9XYZ RR73", MessageKind::Signoff).is_none());
        assert!(decoded_location("CQ W9XYZ EN37", MessageKind::Other).is_none());
    }
}
