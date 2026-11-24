class Fetcher
  def fetch(key, retries: 3)
    [key, retries]
  end

  def go(x)
    x.instance_eval { fetch(1, retries: 3) }
  end
end
