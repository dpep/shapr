class Fetcher
  def fetch(key, retries: 3)
    [key, retries]
  end

  def go(x)
    x.each { |k| fetch(k) }
  end
end
