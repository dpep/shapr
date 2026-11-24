class Fetcher
  def fetch(key, retries: 3)
    [key, retries]
  end

  def go(x)
    y = fetch(x, retries: 3)
  end
end
