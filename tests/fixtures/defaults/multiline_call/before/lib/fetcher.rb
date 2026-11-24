class Fetcher
  def fetch(key, retries: 3)
    [key, retries]
  end

  def go(x)
    fetch(
      x,
      retries: 3,
    )
  end
end
