class Store < Base
  def save(record)
    record = record.to_h
    super(record)
  end
end
