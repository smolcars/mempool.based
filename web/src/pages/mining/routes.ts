import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: "/mining", component: lazy(() => import("./MiningPage")) },
  { path: "/mining/pool/:slug", component: lazy(() => import("./PoolPage")) },
];
