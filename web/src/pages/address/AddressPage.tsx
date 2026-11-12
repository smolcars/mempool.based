import { useParams } from "@solidjs/router";
import { Stub } from "../Stub";

export default function AddressPage() {
  const params = useParams<{ address: string }>();
  return (
    <Stub
      title="Address"
      subject={params.address}
      task="W5"
      shows="this address's balance, unspent outputs and history"
    />
  );
}
