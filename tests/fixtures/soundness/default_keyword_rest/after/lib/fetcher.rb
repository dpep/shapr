class Fetcher
  def fetch(key, retries: 3, **opts)
    [key, retries, opts]
  end

  def go(x)
    fetch(x)
  end
end
