import { useParams } from "@solidjs/router";
import { Stub } from "../Stub";

export default function TransactionPage() {
  const params = useParams<{ txid: string }>();
  return (
    <Stub
      title="Transaction"
      subject={params.txid}
      task="W4"
      shows="this transaction's inputs, outputs, fee and status"
    />
  );
}
