"use client";

import { useEffect, useRef, useState } from "react";
import type { GeoPoint, OpsisEvent } from "../lib/types";
import { eventSummary } from "../lib/utils";

// CesiumJS is loaded dynamically to avoid SSR issues.
// Falls back to the CSS globe if Cesium fails to load.

interface GlobeProps {
  events: OpsisEvent[];
  selectedDomain: string | null;
  googleApiKey?: string;
  cesiumIonToken?: string;
}

interface CesiumModules {
  Viewer: typeof import("cesium").Viewer;
  Cesium3DTileset: typeof import("cesium").Cesium3DTileset;
  Cartesian3: typeof import("cesium").Cartesian3;
  Color: typeof import("cesium").Color;
  Entity: typeof import("cesium").Entity;
  Ion: typeof import("cesium").Ion;
  PointGraphics: typeof import("cesium").PointGraphics;
  LabelGraphics: typeof import("cesium").LabelGraphics;
  NearFarScalar: typeof import("cesium").NearFarScalar;
  VerticalOrigin: typeof import("cesium").VerticalOrigin;
  HorizontalOrigin: typeof import("cesium").HorizontalOrigin;
  LabelStyle: typeof import("cesium").LabelStyle;
  RequestScheduler: typeof import("cesium").RequestScheduler;
}

const DOMAIN_COLORS: Record<string, [number, number, number]> = {
  Emergency: [239, 68, 68],
  Health: [34, 197, 94],
  Finance: [59, 130, 246],
  Trade: [245, 158, 11],
  Conflict: [220, 38, 38],
  Politics: [139, 92, 246],
  Weather: [6, 182, 212],
  Space: [167, 139, 250],
  Ocean: [14, 165, 233],
  Technology: [16, 185, 129],
  Personal: [244, 114, 182],
  Infrastructure: [100, 116, 139],
};

export function Globe({ events, selectedDomain, googleApiKey, cesiumIonToken }: GlobeProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [cesiumLoaded, setCesiumLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const viewerRef = useRef<InstanceType<CesiumModules["Viewer"]> | null>(null);
  const cesiumRef = useRef<CesiumModules | null>(null);

  // Dynamic import of CesiumJS.
  useEffect(() => {
    let cancelled = false;

    async function loadCesium() {
      try {
        // Load Cesium CSS.
        if (!document.querySelector('link[href*="cesium/Build"]')) {
          const link = document.createElement("link");
          link.rel = "stylesheet";
          link.href = "https://cesium.com/downloads/cesiumjs/releases/1.131/Build/Cesium/Widgets/widgets.css";
          document.head.appendChild(link);
        }

        const cesium = await import("cesium");
        if (cancelled) return;

        // Configure Cesium base URL for assets.
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (window as any).CESIUM_BASE_URL =
          "https://cesium.com/downloads/cesiumjs/releases/1.131/Build/Cesium/";

        cesiumRef.current = {
          Viewer: cesium.Viewer,
          Cesium3DTileset: cesium.Cesium3DTileset,
          Cartesian3: cesium.Cartesian3,
          Color: cesium.Color,
          Entity: cesium.Entity,
          Ion: cesium.Ion,
          PointGraphics: cesium.PointGraphics,
          LabelGraphics: cesium.LabelGraphics,
          NearFarScalar: cesium.NearFarScalar,
          VerticalOrigin: cesium.VerticalOrigin,
          HorizontalOrigin: cesium.HorizontalOrigin,
          LabelStyle: cesium.LabelStyle,
          RequestScheduler: cesium.RequestScheduler,
        };

        setCesiumLoaded(true);
      } catch (err) {
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : "Failed to load CesiumJS");
        }
      }
    }

    loadCesium();
    return () => { cancelled = true; };
  }, []);

  // Create Cesium Viewer once loaded.
  useEffect(() => {
    if (!cesiumLoaded || !containerRef.current || viewerRef.current) return;
    const C = cesiumRef.current;
    if (!C) return;

    try {
      // Set Ion token for base imagery.
      if (cesiumIonToken) {
        C.Ion.defaultAccessToken = cesiumIonToken;
      }

      // Ensure container has dimensions before Cesium measures it.
      containerRef.current.style.width = "100%";
      containerRef.current.style.height = "100%";

      const viewer = new C.Viewer(containerRef.current, {
        animation: false,
        baseLayerPicker: false,
        fullscreenButton: false,
        geocoder: false,
        homeButton: false,
        infoBox: false,
        sceneModePicker: false,
        selectionIndicator: false,
        timeline: false,
        navigationHelpButton: false,
        orderIndependentTranslucency: false,
        msaaSamples: 4,
      });

      // Dark space styling to match Opsis aesthetic.
      viewer.scene.backgroundColor = C.Color.fromCssColorString("#060a14");
      viewer.scene.globe.enableLighting = true;
      // biome-ignore lint/suspicious/noExplicitAny: Cesium scene props vary by version
      const scene = viewer.scene as any;
      if (scene.sun) scene.sun.show = false;
      if (scene.moon) scene.moon.show = false;
      if (scene.skyBox) scene.skyBox.show = false;
      if (scene.skyAtmosphere) scene.skyAtmosphere.show = true;

      // Remove default Cesium chrome.
      const creditContainer = viewer.cesiumWidget.creditContainer as HTMLElement;
      creditContainer.style.display = "none";

      // Force resize to fill container.
      viewer.resize();

      // Load Google 3D Tiles if API key provided.
      if (googleApiKey) {
        C.RequestScheduler.requestsByServer["tile.googleapis.com:443"] = 18;
        C.Cesium3DTileset.fromUrl(
          `https://tile.googleapis.com/v1/3dtiles/root.json?key=${googleApiKey}`,
          { showCreditsOnScreen: true },
        ).then((tileset) => {
          viewer.scene.primitives.add(tileset);
        }).catch(() => {
          // Google tiles unavailable — use default globe.
        });
      }

      viewerRef.current = viewer;
    } catch {
      setLoadError("Failed to initialize Cesium viewer");
    }

    return () => {
      if (viewerRef.current && !viewerRef.current.isDestroyed()) {
        viewerRef.current.destroy();
        viewerRef.current = null;
      }
    };
  }, [cesiumLoaded, googleApiKey, cesiumIonToken]);

  // Update event markers on the globe.
  useEffect(() => {
    const viewer = viewerRef.current;
    const C = cesiumRef.current;
    if (!viewer || !C || viewer.isDestroyed()) return;

    viewer.entities.removeAll();

    const filtered = selectedDomain
      ? events.filter((e) => e.domain === selectedDomain)
      : events;

    const located = filtered.filter((e) => e.location).slice(-200);

    for (const event of located) {
      if (!event.location) continue;
      const rgb = DOMAIN_COLORS[(event.domain ?? "System")] ?? [100, 116, 139];
      const alpha = 0.4 + (event.severity ?? 0) * 0.6;
      const size = 4 + (event.severity ?? 0) * 12;

      // Use any cast — Cesium Entity constructor types are complex with dynamic imports.
      // biome-ignore lint/suspicious/noExplicitAny: Cesium dynamic API
      const entityOpts: any = {
        position: C.Cartesian3.fromDegrees(event.location.lon, event.location.lat),
        point: {
          pixelSize: size,
          color: C.Color.fromBytes(rgb[0]!, rgb[1]!, rgb[2]!, Math.round(alpha * 255)),
          outlineWidth: (event.severity ?? 0) >= 0.7 ? 1 : 0,
          outlineColor: C.Color.fromBytes(rgb[0]!, rgb[1]!, rgb[2]!, 100),
        },
      };

      if ((event.severity ?? 0) >= 0.5) {
        entityOpts.label = {
          text: eventSummary(event).slice(0, 40),
          font: "10px monospace",
          fillColor: C.Color.fromBytes(200, 200, 200, 200),
          outlineColor: C.Color.BLACK,
          outlineWidth: 2,
          style: C.LabelStyle.FILL_AND_OUTLINE,
          verticalOrigin: C.VerticalOrigin.BOTTOM,
          scaleByDistance: new C.NearFarScalar(1e3, 1.0, 1e7, 0.3),
        };
      }

      viewer.entities.add(entityOpts);
    }

    viewer.scene.requestRender();
  }, [events, selectedDomain]);

  // Fallback if Cesium can't load.
  if (loadError || !cesiumLoaded) {
    return <FallbackGlobe events={events} selectedDomain={selectedDomain} error={loadError} />;
  }

  return (
    <div
      ref={containerRef}
      className="absolute inset-0"
      style={{ background: "#0a0e17", width: "100%", height: "100%" }}
    />
  );
}

/** CSS-only fallback globe when CesiumJS fails to load. */
function FallbackGlobe({
  events,
  selectedDomain,
  error,
}: {
  events: OpsisEvent[];
  selectedDomain: string | null;
  error: string | null;
}) {
  const filtered = selectedDomain
    ? events.filter((e) => e.domain === selectedDomain)
    : events;
  const located = filtered.filter((e) => e.location).slice(-300);

  return (
    <div className="absolute inset-0 bg-[var(--color-bg-space)]">
      {/* Globe gradient sphere */}
      <div className="absolute inset-0 flex items-center justify-center" style={{ marginTop: "-5%" }}>
        <div
          className="rounded-full"
          style={{
            width: "min(70vh, 70vw)",
            height: "min(70vh, 70vw)",
            background:
              "radial-gradient(ellipse at 40% 35%, oklch(0.18 0.03 220) 0%, oklch(0.12 0.02 230) 40%, oklch(0.08 0.01 250) 70%, transparent 100%)",
            boxShadow:
              "0 0 60px oklch(0.15 0.04 220 / 0.3), inset 0 0 40px oklch(0.06 0.01 250 / 0.5)",
          }}
        />
      </div>

      {/* Atmosphere glow */}
      <div
        className="absolute inset-0 flex items-center justify-center pointer-events-none"
        style={{ marginTop: "-5%" }}
      >
        <div
          className="rounded-full"
          style={{
            width: "min(73vh, 73vw)",
            height: "min(73vh, 73vw)",
            background:
              "radial-gradient(ellipse at 40% 35%, oklch(0.30 0.08 210 / 0.08) 60%, transparent 100%)",
          }}
        />
      </div>

      {/* Event dots */}
      <div className="absolute inset-0 pointer-events-none">
        {located.map((event) => {
          if (!event.location) return null;
          const x = ((event.location.lon + 180) / 360) * 100;
          const y = ((90 - event.location.lat) / 180) * 100;
          const size = 3 + (event.severity ?? 0) * 10;
          const domainRgb = DOMAIN_COLORS[(event.domain ?? "System")];
          const color = domainRgb ? `rgb(${domainRgb.join(",")})` : "#64748b";

          return (
            <div
              key={event.id}
              className="absolute rounded-full"
              style={{
                left: `${15 + x * 0.7}%`,
                top: `${10 + y * 0.65}%`,
                width: size,
                height: size,
                backgroundColor: color,
                opacity: 0.4 + (event.severity ?? 0) * 0.6,
                boxShadow:
                  (event.severity ?? 0) >= 0.6 ? `0 0 ${size * 2}px ${color}80` : undefined,
                transform: "translate(-50%, -50%)",
              }}
            />
          );
        })}
      </div>

      {/* Status text */}
      <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
        <div className="text-center" style={{ marginTop: "-5%" }}>
          {located.length === 0 && (
            <>
              <p className="text-[var(--color-text-muted)] text-xs tracking-wider">
                AWAITING WORLD STATE
              </p>
              <p className="text-[var(--color-text-muted)] text-[10px] mt-1 opacity-50">
                cargo run -p opsisd
              </p>
            </>
          )}
          {error && (
            <p className="text-[var(--color-text-muted)] text-[9px] mt-2 opacity-30">
              CesiumJS: {error}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
