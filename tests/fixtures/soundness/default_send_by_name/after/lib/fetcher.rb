class Fetcher
  def fetch(key, retries: 3)
    [key, retries]
  end

  def go(x)
    fetch(x)
  end

  def other
    send(:fetch, 1)
  end
end
