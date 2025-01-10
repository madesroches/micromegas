import datetime
import pandas as pd
import micromegas

client = micromegas.connect()

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=10000)
end = now + datetime.timedelta(hours=1)
limit = 1024
