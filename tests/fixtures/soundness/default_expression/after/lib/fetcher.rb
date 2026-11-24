class Fetcher
  RETRIES = 3

  def fetch(key, retries: RETRIES)
    [key, retries]
  end

  def go(x)
    fetch(x)
  end
end
