"""boto3 API calls to S3, DynamoDB and SQS, answered by canned HTTP responses.

Stands for the most-installed Python library (boto3/botocore, the AWS SDK: the #1 and #2
packages on PyPI by downloads) as real programs use it: a data service that lists and
reads S3 objects, writes and queries DynamoDB items through the boto3 resource layer
(``Table``, ``Key(...)`` condition expressions, ``TypeSerializer``/``TypeDeserializer``
with ``Decimal``), and moves messages through SQS. Every call runs botocore's full
client path: parameter validation against the service model, the event system
(dozens of handlers per call), endpoint-ruleset resolution, request serialization
(REST-XML, JSON), SigV4 signing (canonical request, HMAC-SHA256 chain), checksums, retry
bookkeeping, then response parsing (XML via ElementTree, JSON, timestamps, ``x-amz-crc32``
validation, error-code mapping to ``ClientError``). Only the socket is replaced: a
``before-send`` handler returns a canned ``AWSResponse`` (the hook botocore's own
``Stubber`` and moto build on), so no network is used. Clients are created once in
setup, as long-running programs do. 1,140 API calls per run.
"""

REQUIRES = ["boto3==1.43.123", "botocore==1.43.123", "s3transfer==0.19.3", "jmespath==1.1.0",
            "python-dateutil==2.9.0.post0", "six==1.17.0", "urllib3==2.8.0"]


def setup():
    import base64
    import datetime as dt
    import hashlib
    import json
    import os
    import random
    import zlib
    from decimal import Decimal
    from urllib.parse import parse_qs, urlsplit

    # no shared config/credential files or instance metadata: everything is explicit
    os.environ["AWS_CONFIG_FILE"] = os.devnull
    os.environ["AWS_SHARED_CREDENTIALS_FILE"] = os.devnull
    os.environ["AWS_EC2_METADATA_DISABLED"] = "true"

    import boto3
    from botocore.awsrequest import AWSResponse
    from botocore.config import Config
    from urllib3 import HTTPHeaderDict

    rng = random.Random(31)

    class Raw:
        def __init__(self, body):
            self.body = body

        def stream(self, **kwargs):
            yield self.body

        def read(self, amt=None, **kwargs):
            b, self.body = self.body, b""
            return b

    def response(url, status, headers, body):
        h = HTTPHeaderDict({"x-amz-request-id": "4442587FB7D0A2F9", "date": "Mon, 03 Mar 2025 10:00:00 GMT",
                            "content-length": str(len(body))})
        h.update(headers)
        return AWSResponse(url, status, h, Raw(body))

    # ---- canned S3 responses ------------------------------------------------------------
    keys = [f"logs/2025/03/{d:02d}/app-{i:05d}.json.gz" for d in range(1, 11) for i in range(100)]
    def list_xml(prefix, start, n, token):
        items = "".join(
            f"<Contents><Key>{k}</Key><LastModified>2025-03-{1 + j % 28:02d}T10:{j % 60:02d}:00.000Z</LastModified>"
            f"<ETag>&quot;{hashlib.md5(k.encode()).hexdigest()}&quot;</ETag><ChecksumAlgorithm>CRC32</ChecksumAlgorithm>"
            f"<Size>{1000 + (j * 7919) % 90000}</Size><StorageClass>STANDARD</StorageClass></Contents>"
            for j, k in enumerate(keys[start:start + n]))
        more = f"<NextContinuationToken>{token}</NextContinuationToken>" if token else ""
        return ('<?xml version="1.0" encoding="UTF-8"?>\n<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">'
                f"<Name>data-bucket</Name><Prefix>{prefix}</Prefix><KeyCount>{n}</KeyCount><MaxKeys>{n}</MaxKeys>"
                f"<IsTruncated>{'true' if token else 'false'}</IsTruncated>{more}{items}</ListBucketResult>").encode()

    list_pages = [list_xml("logs/", p * 100, 100, f"tok{p + 1}" if p < 9 else None) for p in range(10)]
    obj_body = json.dumps([{"ts": f"2025-03-01T10:{i % 60:02d}:00Z", "level": "INFO", "msg": f"event {i}"}
                           for i in range(40)]).encode()
    obj_headers = {"content-type": "application/json", "etag": '"%s"' % hashlib.md5(obj_body).hexdigest(),
                   "last-modified": "Sat, 01 Mar 2025 10:00:00 GMT", "x-amz-meta-source": "ingest",
                   "accept-ranges": "bytes", "x-amz-version-id": "3HL4kqtJlcpXroDTDmJ"}
    no_such_key = (b'<?xml version="1.0" encoding="UTF-8"?>\n<Error><Code>NoSuchKey</Code><Message>The specified key '
                   b"does not exist.</Message><Key>missing.json</Key><RequestId>4442587FB7D0A2F9</RequestId></Error>")
    delete_result = ('<?xml version="1.0" encoding="UTF-8"?>\n<DeleteResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">'
                     + "".join(f"<Deleted><Key>{k}</Key></Deleted>" for k in keys[:20]) + "</DeleteResult>").encode()

    # ---- canned DynamoDB responses ------------------------------------------------------
    def ddb_item(i):
        return {"pk": {"S": f"customer#{i % 50:03d}"}, "sk": {"S": f"order#{i:06d}"},
                "total": {"N": str(Decimal(rng.randint(100, 99999)) / 100)}, "status": {"S": rng.choice(["new", "paid", "sent"])},
                "items": {"L": [{"M": {"sku": {"S": f"SKU-{rng.randint(1, 999):03d}"}, "qty": {"N": str(rng.randint(1, 5))},
                                       "price": {"N": f"{rng.randint(100, 9999) / 100:.2f}"}}} for _ in range(rng.randint(1, 4))]},
                "tags": {"SS": sorted(rng.sample(["gift", "express", "b2b", "promo", "eu"], 2))},
                "created": {"S": f"2025-03-{1 + i % 28:02d}T12:00:00Z"}, "flags": {"M": {"fraud_check": {"BOOL": i % 9 == 0},
                                                                                         "note": {"NULL": True}}}}

    def ddb(body_obj):
        body = json.dumps(body_obj, separators=(",", ":")).encode()
        return body, {"content-type": "application/x-amz-json-1.0", "x-amz-crc32": str(zlib.crc32(body))}

    query_pages = [ddb({"Items": [ddb_item(p * 25 + j) for j in range(25)], "Count": 25, "ScannedCount": 25,
                        **({"LastEvaluatedKey": {"pk": {"S": "customer#001"}, "sk": {"S": f"order#{p * 25 + 24:06d}"}}}
                           if p < 3 else {})}) for p in range(4)]
    get_item = ddb({"Item": ddb_item(7)})
    empty_ddb = ddb({})
    batch_ok = ddb({"UnprocessedItems": {}})
    cond_fail = ddb({"__type": "com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException",
                     "message": "The conditional request failed"})

    # ---- canned SQS responses -----------------------------------------------------------
    def sqs_receive(n, base):
        msgs = []
        for j in range(n):
            body = json.dumps({"job": "resize", "id": base + j, "src": f"s3://data-bucket/img/{base + j}.png"})
            msgs.append({"MessageId": f"5fea7756-0ea4-451a-a703-{base + j:012d}", "ReceiptHandle": base64.b64encode(
                f"rh-{base + j}".encode() * 4).decode(), "MD5OfBody": hashlib.md5(body.encode()).hexdigest(),
                "Body": body, "Attributes": {"SentTimestamp": "1741000000000", "ApproximateReceiveCount": "1"}})
        return json.dumps({"Messages": msgs}).encode()

    receive_bodies = [sqs_receive(10, 100 * k) for k in range(5)]

    def sqs_send_batch(request):
        entries = json.loads(request.body)["Entries"]
        return json.dumps({"Successful": [{"Id": e["Id"], "MessageId": f"m-{e['Id']}",
                                           "MD5OfMessageBody": hashlib.md5(e["MessageBody"].encode()).hexdigest()}
                                          for e in entries], "Failed": []}).encode()

    session = boto3.session.Session(aws_access_key_id="AKIAIOSFODNN7EXAMPLE",
                                    aws_secret_access_key="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
                                    region_name="eu-central-1")
    cfg = Config(retries={"mode": "standard", "max_attempts": 1})
    s3 = session.client("s3", config=cfg)
    sqs = session.client("sqs", config=cfg)
    dynamo = session.resource("dynamodb", config=cfg)
    table = dynamo.Table("orders")
    state = {"page": 0}

    def s3_send(request, **kwargs):
        op = kwargs["event_name"].rsplit(".", 1)[1]
        url = request.url
        if op == "ListObjectsV2":
            tok = parse_qs(urlsplit(url).query).get("continuation-token", ["tok0"])[0]
            tok = int(tok[3:])
            return response(url, 200, {"content-type": "application/xml"}, list_pages[tok])
        if op in ("GetObject", "HeadObject"):
            if "missing" in url:
                return response(url, 404, {"content-type": "application/xml"}, no_such_key if op == "GetObject" else b"")
            return response(url, 200, obj_headers, obj_body if op == "GetObject" else b"")
        if op == "PutObject":
            return response(url, 200, {"etag": '"%s"' % hashlib.md5(b"x").hexdigest(),
                                       "x-amz-checksum-crc32": "AAAAAA==", "x-amz-server-side-encryption": "AES256"}, b"")
        if op == "DeleteObjects":
            return response(url, 200, {"content-type": "application/xml"}, delete_result)
        raise AssertionError(op)

    def ddb_send(request, **kwargs):
        op = kwargs["event_name"].rsplit(".", 1)[1]
        if op == "Query":
            body, hdr = query_pages[state["page"] % 4]
            state["page"] += 1
        elif op == "GetItem":
            body, hdr = get_item
        elif op == "PutItem":
            if b"attribute_not_exists" in request.body and b"order#999999" in request.body:
                body, hdr = cond_fail
                return response(request.url, 400, hdr, body)
            body, hdr = empty_ddb
        elif op == "BatchWriteItem":
            body, hdr = batch_ok
        else:
            raise AssertionError(op)
        return response(request.url, 200, hdr, body)

    def sqs_send(request, **kwargs):
        op = kwargs["event_name"].rsplit(".", 1)[1]
        hdr = {"content-type": "application/x-amz-json-1.0"}
        if op == "SendMessageBatch":
            return response(request.url, 200, hdr, sqs_send_batch(request))
        if op == "ReceiveMessage":
            state["recv"] = state.get("recv", -1) + 1
            return response(request.url, 200, hdr, receive_bodies[state["recv"] % 5])
        if op == "DeleteMessageBatch":
            ids = [e["Id"] for e in json.loads(request.body)["Entries"]]
            return response(request.url, 200, hdr, json.dumps({"Successful": [{"Id": i} for i in ids], "Failed": []}).encode())
        raise AssertionError(op)

    s3.meta.events.register("before-send.s3", s3_send)
    dynamo.meta.client.meta.events.register("before-send.dynamodb", ddb_send)
    sqs.meta.events.register("before-send.sqs", sqs_send)
    orders = [{"pk": f"customer#{i % 50:03d}", "sk": f"order#{i:06d}", "total": Decimal(f"{i % 997}.{i % 100:02d}"),
               "status": "new", "items": [{"sku": f"SKU-{i % 999:03d}", "qty": 1 + i % 3, "price": Decimal("9.99")}],
               "tags": {"gift", "eu"} if i % 2 else {"b2b"}, "created": dt.datetime(2025, 3, 1, 12, i % 60).isoformat(),
               "flags": {"fraud_check": i % 9 == 0, "note": None}} for i in range(400)]
    return {"s3": s3, "sqs": sqs, "table": table, "state": state, "orders": orders}


def run(state):
    import json

    from boto3.dynamodb.conditions import Attr, Key
    from botocore.exceptions import ClientError

    s3, sqs, table, orders = state["s3"], state["sqs"], state["table"], state["orders"]
    state["state"]["page"] = 0
    state["state"]["recv"] = -1
    queue = "https://sqs.eu-central-1.amazonaws.com/123456789012/jobs"
    out = []

    def clean(resp):
        resp = dict(resp)
        resp.pop("ResponseMetadata", None)
        if "Body" in resp:
            resp["Body"] = resp["Body"].read()
        return resp

    for rnd in range(20):
        # S3: paginate a listing, read and stat objects, write, a missing key, batch delete
        paginator = s3.get_paginator("list_objects_v2")
        sizes = 0
        for page in paginator.paginate(Bucket="data-bucket", Prefix="logs/", PaginationConfig={"PageSize": 100}):
            sizes += sum(o["Size"] for o in page["Contents"])
            out.append((page["KeyCount"], page["Contents"][-1]["Key"], page["Contents"][0]["LastModified"].isoformat()))
        out.append(sizes)
        for j in range(6):
            r = s3.get_object(Bucket="data-bucket", Key=f"logs/2025/03/01/app-{rnd * 6 + j:05d}.json.gz")
            out.append((len(json.loads(r["Body"].read())), r["ETag"], r["Metadata"], r["LastModified"].isoformat()))
            h = s3.head_object(Bucket="data-bucket", Key=f"logs/2025/03/02/app-{j:05d}.json.gz")
            out.append((h["ContentLength"], h["VersionId"]))
            s3.put_object(Bucket="data-bucket", Key=f"out/{rnd}/{j}.json", Body=json.dumps({"r": rnd, "j": j}).encode(),
                          ContentType="application/json", Metadata={"job": str(j)})
        try:
            s3.get_object(Bucket="data-bucket", Key="missing.json")
        except ClientError as e:
            out.append((e.response["Error"]["Code"], e.response["ResponseMetadata"]["HTTPStatusCode"]))
        out.append(clean(s3.delete_objects(Bucket="data-bucket", Delete={"Objects": [
            {"Key": f"tmp/{rnd}/{k}"} for k in range(20)], "Quiet": False}))["Deleted"][-1])
        # DynamoDB via the resource layer
        for j in range(10):
            table.put_item(Item=orders[(rnd * 10 + j) % 400], ConditionExpression=Attr("sk").not_exists())
        try:
            table.put_item(Item=dict(orders[0], sk="order#999999"), ConditionExpression="attribute_not_exists(sk)")
        except ClientError as e:
            out.append(e.response["Error"]["Code"])
        for j in range(4):
            r = table.query(KeyConditionExpression=Key("pk").eq(f"customer#{j:03d}") & Key("sk").begins_with("order#"),
                            FilterExpression=Attr("status").is_in(["new", "paid"]) & Attr("total").gt(1),
                            Limit=25, ScanIndexForward=False)
            out.append((r["Count"], sum(it["total"] for it in r["Items"]), r["Items"][0]["items"][0]["qty"],
                        sorted(r["Items"][-1]["tags"])))
        r = table.get_item(Key={"pk": "customer#007", "sk": "order#000007"}, ConsistentRead=True)
        out.append(repr(r["Item"]["flags"]))
        with table.batch_writer() as bw:
            for j in range(30):
                bw.put_item(Item=orders[(rnd * 30 + j) % 400])
        # SQS
        for j in range(3):
            sqs.send_message_batch(QueueUrl=queue, Entries=[
                {"Id": f"m{k}", "MessageBody": json.dumps({"job": "resize", "n": rnd * 100 + j * 10 + k}),
                 "MessageAttributes": {"tenant": {"DataType": "String", "StringValue": "acme"}}} for k in range(10)])
            r = sqs.receive_message(QueueUrl=queue, MaxNumberOfMessages=10, WaitTimeSeconds=0,
                                    MessageSystemAttributeNames=["All"])
            out.append([json.loads(m["Body"])["id"] for m in r["Messages"]])
            sqs.delete_message_batch(QueueUrl=queue, Entries=[
                {"Id": str(k), "ReceiptHandle": m["ReceiptHandle"]} for k, m in enumerate(r["Messages"])])
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
