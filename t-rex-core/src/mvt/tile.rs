//
// Copyright (c) Pirmin Kalberer. All rights reserved.
// Licensed under the MIT License. See LICENSE file in the project root for full license information.
//

use crate::core::feature::{Feature, FeatureAttrValType};
use crate::core::layer::Layer;
use crate::core::screen;
use crate::core::{geom, geom::GeometryType};
use crate::mvt::geom_encoder::{CommandSequence, EncodableGeom};
use crate::mvt::vector_tile;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use protobuf::{error::ProtobufError, CodedOutputStream, Message};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use tile_grid::Extent;

pub struct Tile<'a> {
    pub mvt_tile: vector_tile::Tile,
    extent: &'a Extent,
    reverse_y: bool,
    // Values for current layer
    tile_size: i32,
    buffer_size: i32,
    pixel_size_x: f64,
    pixel_size_y: f64,
}

impl GeometryType {
    /// GeometryType to MVT geom type
    pub fn mvt_field_type(&self) -> vector_tile::Tile_GeomType {
        match self {
            &GeometryType::Point(_) => vector_tile::Tile_GeomType::POINT,
            &GeometryType::LineString(_) => vector_tile::Tile_GeomType::LINESTRING,
            &GeometryType::Polygon(_) => vector_tile::Tile_GeomType::POLYGON,
            &GeometryType::MultiPoint(_) => vector_tile::Tile_GeomType::POINT,
            &GeometryType::MultiLineString(_) => vector_tile::Tile_GeomType::LINESTRING,
            &GeometryType::MultiPolygon(_) => vector_tile::Tile_GeomType::POLYGON,
            &GeometryType::GeometryCollection(_) => vector_tile::Tile_GeomType::UNKNOWN,
            &GeometryType::Geometry(_) => vector_tile::Tile_GeomType::UNKNOWN,
        }
    }
}

pub trait ScreenGeom<T> {
    /// Convert geometry into screen coordinates
    fn from_geom(tile: &Tile, geom: &T) -> Self;
}

impl ScreenGeom<geom::MultiPoint> for screen::MultiPoint {
    fn from_geom(tile: &Tile, multipoint: &geom::MultiPoint) -> Self {
        let mut screen_geom = screen::MultiPoint {
            points: Vec::with_capacity(multipoint.points.len()),
        };
        for point in &multipoint.points {
            let pt = tile.point(point);
            if tile.point_in_buffer(&pt) {
                screen_geom.points.push(pt);
            }
        }
        screen_geom
    }
}

impl ScreenGeom<geom::LineString> for screen::LineString {
    fn from_geom(tile: &Tile, line: &geom::LineString) -> Self {
        let mut screen_geom = screen::LineString {
            points: Vec::with_capacity(line.points.len()),
        };
        for point in &line.points {
            let pt = tile.point(point);
            screen_geom.points.push(pt);
        }
        screen_geom.points.dedup();
        screen_geom
    }
}

impl ScreenGeom<geom::MultiLineString> for screen::MultiLineString {
    fn from_geom(tile: &Tile, multiline: &geom::MultiLineString) -> Self {
        let mut screen_geom = screen::MultiLineString {
            lines: Vec::with_capacity(multiline.lines.len()),
        };
        for line in &multiline.lines {
            screen_geom
                .lines
                .push(screen::LineString::from_geom(tile, line));
        }
        screen_geom
    }
}

impl ScreenGeom<geom::Polygon> for screen::Polygon {
    fn from_geom(tile: &Tile, polygon: &geom::Polygon) -> Self {
        let mut screen_geom = screen::Polygon {
            rings: Vec::with_capacity(polygon.rings.len()),
        };
        for line in &polygon.rings {
            screen_geom
                .rings
                .push(screen::LineString::from_geom(tile, line));
        }
        screen_geom
    }
}

impl ScreenGeom<geom::MultiPolygon> for screen::MultiPolygon {
    fn from_geom(tile: &Tile, multipolygon: &geom::MultiPolygon) -> Self {
        let mut screen_geom = screen::MultiPolygon {
            polygons: Vec::with_capacity(multipolygon.polygons.len()),
        };
        for polygon in &multipolygon.polygons {
            screen_geom
                .polygons
                .push(screen::Polygon::from_geom(tile, polygon));
        }
        screen_geom
    }
}

// --- Tile creation functions

impl<'a> Tile<'a> {
    pub fn new(extent: &Extent, reverse_y: bool) -> Tile {
        let mvt_tile = vector_tile::Tile::new();
        let mut tile = Tile {
            mvt_tile,
            extent,
            reverse_y,
            tile_size: 0,
            buffer_size: 0,
            pixel_size_x: 0.0,
            pixel_size_y: 0.0,
        };
        let default_layer = Layer::new("");
        tile.calc_layer_values(&default_layer);
        tile
    }

    pub fn new_layer(&mut self, layer: &Layer) -> vector_tile::Tile_Layer {
        self.calc_layer_values(layer);

        let mut mvt_layer = vector_tile::Tile_Layer::new();
        mvt_layer.set_version(2);
        mvt_layer.set_name(layer.name.clone());
        mvt_layer.set_extent(layer.tile_size);
        mvt_layer
    }

    fn calc_layer_values(&mut self, layer: &Layer) {
        self.tile_size = layer.tile_size as i32;
        self.buffer_size = layer.buffer_size.unwrap_or(0) as i32;
        self.pixel_size_x = (self.extent.maxx - self.extent.minx) / self.tile_size as f64;
        self.pixel_size_y = (self.extent.maxy - self.extent.miny) / self.tile_size as f64;
        //println!("\n\n\n************************\ndebug calc_layer_values s={} b={} x={} y={}",self.tile_size,self.buffer_size,self.pixel_size_x,self.pixel_size_y);
    }

    pub fn point(&self, point: &geom::Point) -> screen::Point {
        let mut screen_geom = screen::Point {
            x: ((point.x - self.extent.minx) / self.pixel_size_x) as i32,
            y: ((point.y - self.extent.miny) / self.pixel_size_y) as i32,
        };
         println!("\n\n\n===================================\ndebug point {} {}",screen_geom.x,screen_geom.y);

        if self.reverse_y {
            screen_geom.y = self.tile_size.saturating_sub(screen_geom.y)
        }

        screen_geom
    }

    pub fn point_in_buffer(&self, point: &screen::Point) -> bool {
        point.x >= -self.buffer_size
            && point.x <= self.tile_size + self.buffer_size
            && point.y >= -self.buffer_size
            && point.y <= self.tile_size + self.buffer_size
    }

    pub fn encode_geom(&self, geom: geom::GeometryType) -> CommandSequence {
        match geom {
            GeometryType::Point(ref g) => {
                let pt = self.point(g);
                if self.point_in_buffer(&pt) {
                    pt.encode()
                } else {
                    CommandSequence::new() // empty
                }
            }
            GeometryType::MultiPoint(ref g) => screen::MultiPoint::from_geom(&self, g).encode(),
            GeometryType::LineString(ref g) => screen::LineString::from_geom(&self, g).encode(),
            GeometryType::MultiLineString(ref g) => {
                screen::MultiLineString::from_geom(&self, g).encode()
            }
            GeometryType::Polygon(ref g) => screen::Polygon::from_geom(&self, g).encode(),
            GeometryType::MultiPolygon(ref g) => screen::MultiPolygon::from_geom(&self, g).encode(),
            GeometryType::GeometryCollection(_) => panic!("GeometryCollection not supported"),
            GeometryType::Geometry(_) => panic!("Geometry not supported"),
        }
    }

    pub fn add_feature_attribute(
        mvt_layer: &mut vector_tile::Tile_Layer,
        mvt_feature: &mut vector_tile::Tile_Feature,
        key: String,
        mvt_value: vector_tile::Tile_Value,
    ) {
        let keyentry = mvt_layer.get_keys().iter().position(|k| *k == key);
        // Optimization: maintain a hash table with key/index pairs
        let keyidx = match keyentry {
            None => {
                mvt_layer.mut_keys().push(key);
                mvt_layer.get_keys().len() - 1
            }
            Some(idx) => idx,
        };
        mvt_feature.mut_tags().push(keyidx as u32);

        let valentry = mvt_layer.get_values().iter().position(|v| *v == mvt_value);
        // Optimization: maintain a hash table with value/index pairs
        let validx = match valentry {
            None => {
                mvt_layer.mut_values().push(mvt_value);
                mvt_layer.get_values().len() - 1
            }
            Some(idx) => idx,
        };
        mvt_feature.mut_tags().push(validx as u32);
    }

    pub fn add_feature(&self, mut mvt_layer: &mut vector_tile::Tile_Layer, feature: &dyn Feature) {
        let mut mvt_feature = vector_tile::Tile_Feature::new();
        if let Some(fid) = feature.fid() {
            mvt_feature.set_id(fid);
        }
        'attr: for attr in feature.attributes() {
            let mut mvt_value = vector_tile::Tile_Value::new();
            match attr.value {
                FeatureAttrValType::String(ref v) => {
                    mvt_value.set_string_value(v.clone());
                }
                FeatureAttrValType::Double(v) => {
                    mvt_value.set_double_value(v);
                }
                FeatureAttrValType::Float(v) => {
                    mvt_value.set_float_value(v);
                }
                FeatureAttrValType::Int(v) => {
                    mvt_value.set_int_value(v);
                }
                FeatureAttrValType::UInt(v) => {
                    mvt_value.set_uint_value(v);
                }
                FeatureAttrValType::SInt(v) => {
                    mvt_value.set_sint_value(v);
                }
                FeatureAttrValType::Bool(v) => {
                    mvt_value.set_bool_value(v);
                }
                FeatureAttrValType::VarcharArray(v) => {
                    for array_val in v {
                        Tile::add_feature_attribute(
                            &mut mvt_layer,
                            &mut mvt_feature,
                            format!("{}.{}", attr.key, array_val),
                            mvt_value.clone(),
                        );
                    }
                    continue 'attr;
                }
            }
            Tile::add_feature_attribute(
                &mut mvt_layer,
                &mut mvt_feature,
                attr.key.clone(),
                mvt_value,
            );
        }
        if let Ok(geom) = feature.geometry() {
            let g_type = geom.mvt_field_type();
            let enc_geom = self.encode_geom(geom).vec();
            if !enc_geom.is_empty() {
                mvt_feature.set_field_type(g_type);
                mvt_feature.set_geometry(enc_geom);
                mvt_layer.mut_features().push(mvt_feature);
            }
        }
    }

    pub fn add_layer(&mut self, mvt_layer: vector_tile::Tile_Layer) {
        self.mvt_tile.mut_layers().push(mvt_layer);
    }

    pub fn write_to(mut out: &mut dyn Write, mvt_tile: &vector_tile::Tile) {
        let mut os = CodedOutputStream::new(&mut out);
        let _ = mvt_tile.write_to(&mut os);
        os.flush().unwrap();
    }

    pub fn write_gz_to(out: &mut dyn Write, mvt_tile: &vector_tile::Tile) {
        let mut gz = GzEncoder::new(out, Compression::default());
        {
            let mut os = CodedOutputStream::new(&mut gz);
            let _ = mvt_tile.write_to(&mut os);
            os.flush().unwrap();
        }
        let _ = gz.finish();
    }

    pub fn read_from(fin: &mut dyn Read) -> Result<vector_tile::Tile, ProtobufError> {
        let mut reader = BufReader::new(fin);
        vector_tile::Tile::parse_from_reader(&mut reader)
    }

    pub fn read_gz_from(fin: &mut dyn Read) -> Result<vector_tile::Tile, ProtobufError> {
        let gz = GzDecoder::new(fin);
        let mut reader = BufReader::new(gz);
        vector_tile::Tile::parse_from_reader(&mut reader)
    }

    pub fn tile_bytevec(mvt_tile: &vector_tile::Tile) -> Vec<u8> {
        let mut v = Vec::with_capacity(mvt_tile.compute_size() as usize);
        Self::write_to(&mut v, mvt_tile);
        v
    }

    pub fn tile_bytevec_gz(mvt_tile: &vector_tile::Tile) -> Vec<u8> {
        let mut v = Vec::with_capacity(mvt_tile.compute_size() as usize);
        Self::write_gz_to(&mut v, &mvt_tile);
        v
    }

    pub fn tile_content(tilegz: Vec<u8>, gzip: bool) -> Vec<u8> {
        if gzip {
            tilegz
        } else {
            let mut gz = GzDecoder::new(&tilegz[..]);
            let mut unc_tile = Vec::with_capacity(tilegz.len());
            let _ = gz.read_to_end(&mut unc_tile);
            unc_tile
        }
    }

    pub fn to_file(&self, fname: &str) {
        let mut f = File::create(fname).unwrap();
        Self::write_to(&mut f, &self.mvt_tile);
    }

    pub fn size(mvt_tile: &vector_tile::Tile) -> u32 {
        mvt_tile.compute_size()
    }
}
