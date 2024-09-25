import datetime
import pandas as pd
import micromegas

BASE_URL = "http://localhost:8082/"
client = micromegas.client.Client(BASE_URL)

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=10000)
end = now + datetime.timedelta(hours=1)
limit = 1024
