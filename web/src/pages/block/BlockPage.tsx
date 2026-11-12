import { useParams } from "@solidjs/router";
import { Stub } from "../Stub";

/** `/block/:id`, where `id` is a block hash or a height. */
export default function BlockPage() {
  const params = useParams<{ id: string }>();
  return (
    <Stub
      title="Block"
      subject={params.id}
      task="W3"
      shows="this block's details and its transactions"
    />
  );
}
