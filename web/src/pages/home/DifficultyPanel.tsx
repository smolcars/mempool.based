import type { Accessor } from "solid-js";
import { DifficultyAdjustment } from "../../components/Difficulty";
import { SectionHeader } from "../../components/SectionHeader";
import type { Network } from "../../lib/network";

/** The difficulty adjustment, as a section of the home page. */
export function DifficultyPanel(props: { network: Network; tip: Accessor<string | undefined> }) {
  return (
    <section class="home-difficulty" aria-labelledby="difficulty-title">
      <SectionHeader title="Difficulty adjustment" id="difficulty-title" />
      <DifficultyAdjustment network={props.network} tip={props.tip} />
    </section>
  );
}
