class Store < Base
  def save(record)
    copy = record
    super(copy)
  end
end
