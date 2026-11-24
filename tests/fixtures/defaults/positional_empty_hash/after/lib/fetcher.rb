class Fetcher
  def merge(base, extra = {})
    base.merge(extra)
  end

  def go(x)
    merge(x)
  end
end
