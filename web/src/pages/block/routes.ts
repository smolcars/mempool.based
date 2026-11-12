import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

export const routes: RouteDefinition[] = [
  { path: "/block/:id", component: lazy(() => import("./BlockPage")) },
];
