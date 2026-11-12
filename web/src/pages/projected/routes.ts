import type { RouteDefinition } from "@solidjs/router";
import { lazy } from "solid-js";

const ProjectedPage = lazy(() => import("./ProjectedPage"));

// Beside `/block/:id`, by place in the queue: the path still means a block to come after one is mined.
export const routes: RouteDefinition[] = [
  { path: ["/block/next", "/block/next/:n"], component: ProjectedPage },
];
