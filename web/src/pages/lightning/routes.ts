import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: "/lightning", component: lazy(() => import("./LightningPage")) },
];
