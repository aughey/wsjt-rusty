// OpenLayers 10.10.0 is shipped locally; only visible OSM tiles use the network.
let map, source;
let latest = {markers: [], links: []}, generation = 0;
let library;
function loadLibrary() {
    if (globalThis.ol) return Promise.resolve(globalThis.ol);
    if (!library) library = new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = new URL('vendor/ol.js', document.baseURI).href;
        script.onload = () => resolve(globalThis.ol);
        script.onerror = () => { script.remove(); library = undefined; reject(new Error('OpenLayers could not load. Close and reopen the map to retry.')); };
        document.head.append(script);
    });
    return library;
}
export async function updateContactMap(json) {
    latest = JSON.parse(json);
    const token = generation;
    try {
        const ol = await loadLibrary();
        const target = document.getElementById('contact-map');
        if (token !== generation || !target || target.parentElement.hidden) return;
        if (!map) {
            source = new ol.source.Vector({wrapX: false});
            const tiles = new ol.source.OSM();
            tiles.on('tileloaderror', () => {
                const status = document.getElementById('contact-map-status');
                if (status) status.textContent = 'Some OpenStreetMap tiles could not load. Station markers remain available.';
            });
            map = new ol.Map({target, layers: [new ol.layer.Tile({source: tiles}), new ol.layer.Vector({source})],
                view: new ol.View({center: [0, 0], zoom: 2, minZoom: 1, maxZoom: 12}),
                controls: ol.control.defaults.defaults({attributionOptions: {collapsible: false}})});
            map.on('singleclick', event => {
                const feature = map.forEachFeatureAtPixel(event.pixel, f => f);
                document.getElementById('contact-map-detail').textContent = feature ? feature.get('detail') : 'Select a station marker.';
            });
        }
        source.clear();
        for (const marker of latest.markers || []) {
            const l = marker.location;
            const feature = new ol.Feature({geometry: new ol.geom.Point(ol.proj.fromLonLat([l.longitude, l.latitude])), detail: `${marker.call} · ${marker.detail}`});
            feature.setStyle(new ol.style.Style({
                image: new ol.style.Circle({radius: 6, fill: new ol.style.Fill({color: marker.color}), stroke: new ol.style.Stroke({color: '#102030', width: 2})}),
                text: new ol.style.Text({text: marker.call, offsetY: -15, fill: new ol.style.Fill({color: '#102030'}), stroke: new ol.style.Stroke({color: '#fff', width: 3})})
            }));
            source.addFeature(feature);
        }
        for (const link of latest.links || []) {
            // Sample the great-circle path and split at the date line, keeping
            // Pacific contacts local instead of drawing across the whole map.
            const parts = communicationPath(link.start, link.end);
            const feature = new ol.Feature({geometry: new ol.geom.MultiLineString(parts.map(part => part.map(p => ol.proj.fromLonLat(p)))),
                detail: `${link.from} ↔ ${link.to} · ${link.inferred ? 'Inferred context / local attempt' : 'Received exchange'} · approximate grid-center path`});
            feature.setStyle(new ol.style.Style({stroke: new ol.style.Stroke({color: link.color, width: 2, lineDash: link.inferred ? [6, 6] : undefined}), zIndex: -1}));
            source.addFeature(feature);
        }
        map.updateSize();
        document.getElementById('contact-map-status').textContent = `${(latest.markers || []).length} located stations · ${(latest.links || []).length} communication paths, including your station when My grid is valid. Locations are approximate.`;
    } catch (error) {
        const status = document.getElementById('contact-map-status');
        if (token === generation && status) status.textContent = error.message;
    }
}
export function disposeContactMap() {
    generation++;
    if (map) { map.setTarget(undefined); map.dispose(); }
    map = source = undefined;
    latest = {markers: [], links: []};
}


// Spherical interpolation in unit vectors, with safe coincident/antipodal cases.
export function communicationPath(start, end) {
    const vector = p => { const lat=p.latitude*Math.PI/180, lon=p.longitude*Math.PI/180; return [Math.cos(lat)*Math.cos(lon),Math.cos(lat)*Math.sin(lon),Math.sin(lat)]; };
    const a=vector(start), b=vector(end);
    const angle=Math.acos(Math.max(-1,Math.min(1,a.reduce((sum,v,i)=>sum+v*b[i],0))));
    const sin=Math.sin(angle), points=[];
    for(let i=0;i<=64;i++) {
        const t=i/64;
        if (Math.abs(sin)<1e-8) { let delta=((end.longitude-start.longitude+540)%360)-180; points.push([((start.longitude+t*delta+540)%360)-180,start.latitude+t*(end.latitude-start.latitude)]); }
        else { const p=a.map((v,j)=>(Math.sin((1-t)*angle)*v+Math.sin(t*angle)*b[j])/sin); points.push([Math.atan2(p[1],p[0])*180/Math.PI,Math.atan2(p[2],Math.hypot(p[0],p[1]))*180/Math.PI]); }
    }
    const parts=[[]];
    for(const p of points) {
        const part=parts[parts.length-1], old=part[part.length-1];
        if(old && Math.abs(p[0]-old[0])>180) {
            const adjusted=p[0]+(p[0]<old[0]?360:-360), edge=old[0]>0?180:-180;
            const lat=old[1]+(p[1]-old[1])*(edge-old[0])/(adjusted-old[0]);
            part.push([edge,lat]); parts.push([[-edge,lat]]);
        }
        parts[parts.length-1].push(p);
    }
    return parts;
}
