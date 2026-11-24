class Fetcher
  def fetch(key, a: 1, retries: 3, b: 2)
    [key, a, retries, b]
  end

  def go(x)
    fetch(x, a: x, retries: 3, b: 5)
  end
end
