use serde::{Deserialize, Serialize};

/// A geographic point in WGS-84 coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    /// Latitude in degrees (−90..90).
    pub lat: f64,
    /// Longitude in degrees (−180..180).
    pub lon: f64,
}

impl GeoPoint {
    /// Create a new `GeoPoint`.
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    /// Haversine distance to another point, in kilometres.
    pub fn distance_km(&self, other: &Self) -> f64 {
        const EARTH_RADIUS_KM: f64 = 6371.0;

        let d_lat = (other.lat - self.lat).to_radians();
        let d_lon = (other.lon - self.lon).to_radians();
        let lat1 = self.lat.to_radians();
        let lat2 = other.lat.to_radians();

        let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().asin();
        EARTH_RADIUS_KM * c
    }
}

/// Axis-aligned bounding box defined by south-west and north-east corners.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bbox {
    /// South-west corner (min lat, min lon).
    pub sw: GeoPoint,
    /// North-east corner (max lat, max lon).
    pub ne: GeoPoint,
}

impl Bbox {
    /// Create a new bounding box.
    pub fn new(sw: GeoPoint, ne: GeoPoint) -> Self {
        Self { sw, ne }
    }

    /// Returns `true` if the given point lies inside (or on the boundary of) this box.
    pub fn contains(&self, point: &GeoPoint) -> bool {
        point.lat >= self.sw.lat
            && point.lat <= self.ne.lat
            && point.lon >= self.sw.lon
            && point.lon <= self.ne.lon
    }
}

/// A geographic hotspot — a concentration of events around a center.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoHotspot {
    /// Center of the hotspot.
    pub center: GeoPoint,
    /// Radius in kilometres.
    pub radius_km: f64,
    /// Normalised intensity (0.0–1.0).
    pub intensity: f32,
    /// Number of events that contributed to this hotspot.
    pub event_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geopoint_distance_same_point_is_zero() {
        let p = GeoPoint::new(4.711, -74.072);
        assert!((p.distance_km(&p) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn geopoint_distance_bogota_to_cali() {
        let bogota = GeoPoint::new(4.711, -74.072);
        let cali = GeoPoint::new(3.4516, -76.532);
        let km = bogota.distance_km(&cali);
        // Roughly ~300 km
        assert!(km > 280.0 && km < 320.0, "distance was {km} km");
    }

    #[test]
    fn bbox_contains_inside_point() {
        let bbox = Bbox::new(GeoPoint::new(0.0, 0.0), GeoPoint::new(10.0, 10.0));
        assert!(bbox.contains(&GeoPoint::new(5.0, 5.0)));
    }

    #[test]
    fn bbox_does_not_contain_outside_point() {
        let bbox = Bbox::new(GeoPoint::new(0.0, 0.0), GeoPoint::new(10.0, 10.0));
        assert!(!bbox.contains(&GeoPoint::new(11.0, 5.0)));
    }
}
